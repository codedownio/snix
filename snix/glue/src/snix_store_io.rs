//! This module provides an implementation of EvalIO talking to snix-store.
use nix_compat::store_path::StorePathRef;
use snix_build::buildservice::BuildService;
use snix_build_glue::build_state::BuildState;
use snix_eval::{EvalIO, FileType, StdIO};
use snix_store::nar::NarCalculationService;
use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    sync::Arc,
};
use tokio_util::io::SyncIoBridge;
use tracing::{Level, error, instrument};
use url::Url;

use snix_castore::{Node, blobservice::BlobService, directoryservice::DirectoryService};
use snix_store::pathinfoservice::{PathInfo, PathInfoService};

/// Implements [EvalIO], asking given [PathInfoService], [DirectoryService]
/// and [BlobService].
///
/// In case the given path does not exist in these stores, we ask StdIO.
/// This is to both cover cases of syntactically valid store paths, that exist
/// on the filesystem (still managed by Nix), as well as being able to read
/// files outside store paths.
///
/// This structure is also directly used by the derivation builtins
/// and tightly coupled to it.
///
/// In the future, we may revisit that coupling and figure out how to generalize this interface and
/// hide this implementation detail of the glue itself so that glue can be used with more than one
/// implementation of "Snix Store IO" which does not necessarily bring the concept of blob service,
/// directory service or path info service.
pub struct SnixStoreIO {
    pub build_state: BuildState,

    std_io: StdIO,
    pub(crate) tokio_handle: tokio::runtime::Handle,

    /// Memoizes positive (store_path, sub_path) lookups: store paths are immutable within
    /// one evaluation, and the eval surface re-resolves the same paths tens of thousands of
    /// times (each one a pathinfo get plus a directory walk).
    lookup_cache: std::cell::RefCell<std::collections::HashMap<([u8; 20], Vec<u8>), PathInfo>>,
}

impl SnixStoreIO {
    pub fn new(
        blob_service: Arc<dyn BlobService>,
        directory_service: Arc<dyn DirectoryService>,
        path_info_service: Arc<dyn PathInfoService>,
        nar_calculation_service: Arc<dyn NarCalculationService>,
        build_service: Arc<dyn BuildService>,
        tokio_handle: tokio::runtime::Handle,
        hashed_mirrors: Vec<Url>,
    ) -> Self {
        Self {
            build_state: BuildState::new(
                blob_service,
                directory_service,
                path_info_service,
                nar_calculation_service,
                build_service,
                hashed_mirrors,
            ),
            std_io: StdIO {},
            tokio_handle,
            lookup_cache: Default::default(),
        }
    }

    /// for a given [StorePath] and additional [snix_castore::Path] inside the store path,
    /// look up the [PathInfo], and if it exists, and then uses
    /// [descend_to] to return the [Node] specified by `sub_path`.
    ///
    /// In case there is no PathInfo yet, we check "self.known_paths" (learnt by
    /// the evaluator)
    ///  - If there's a fetch, we do that and return the resulting PathInfo.
    ///  - If there's a Derivation, we build it and return the resulting
    ///    PathInfo.
    ///    To build it, we need to do a function call to ourselves with all
    ///    inputs of that Derivation, which will trigger fetches / builds of
    ///    inputs, recursively.
    ///
    /// This should be replaced with a proper scheduler knowing about a partial
    /// subgraph at some point, because this design doesn't allow concurrent
    /// builds yet.
    ///
    /// [StorePath]: nix_compat::store_path::StorePath
    /// [descend_to]: snix_castore::directoryservice::traversal::descend_to
    #[instrument(skip(self, store_path), fields(store_path=%store_path, indicatif.pb_show=tracing::field::Empty), ret(level = Level::TRACE), err(level = Level::TRACE))]
    pub(crate) async fn store_path_to_path_info(
        &self,
        store_path: &StorePathRef<'_>,
        sub_path: &snix_castore::Path,
    ) -> io::Result<Option<PathInfo>> {
        let key = (*store_path.digest(), sub_path.as_ref().as_bytes().to_vec());
        if let Some(path_info) = self.lookup_cache.borrow().get(&key) {
            return Ok(Some(path_info.clone()));
        }
        let resolved = self
            .build_state
            .store_path_to_path_info(store_path, sub_path)
            .await?;
        if let Some(path_info) = &resolved {
            self.lookup_cache
                .borrow_mut()
                .insert(key, path_info.clone());
        }
        Ok(resolved)
    }

    /// Re-key an already-in-castore [Node] as a recursive NAR-content-addressed store path and
    /// persist its PathInfo. This is the store-native equivalent of the tail of
    /// [snix_store::import::import_path_as_nar_ca], for when the source content is already in
    /// castore (nothing to ingest from a filesystem).
    async fn node_to_nar_ca_path_info(
        &self,
        name: &str,
        node: Node,
    ) -> io::Result<PathInfo> {
        use nix_compat::nixhash::{CAHash, NixHash};
        let (nar_size, nar_sha256) = self
            .build_state
            .nar_calculation_service
            .calculate_nar(&node)
            .await
            .map_err(io::Error::other)?;
        let hash = NixHash::Sha256(nar_sha256);
        let output_path =
            nix_compat::store_path::build_ca_path(name, true, &hash, [], false).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, format!("invalid name: {name}"))
            })?;
        let path_info = PathInfo {
            store_path: output_path.to_owned(),
            node,
            references: vec![],
            nar_size,
            nar_sha256,
            signatures: vec![],
            deriver: None,
            ca: Some(CAHash::Nar(hash)),
        };
        self.build_state
            .path_info_service
            .as_ref()
            .put(path_info.clone())
            .await
            .map_err(io::Error::other)?;
        Ok(path_info)
    }
}

/// Helper function peeking at a [snix_castore::Node] and returning its [FileType]
fn node_get_type(node: &Node) -> FileType {
    match node {
        Node::Directory { .. } => FileType::Directory,
        Node::File { .. } => FileType::Regular,
        Node::Symlink { .. } => FileType::Symlink,
    }
}

// Helper function converting a [std::path::Path] to a [StorePath] and [snix_castore::Path].
#[cfg(unix)]
pub(crate) fn parse_store_and_sub_path<'a>(
    path: &'a std::path::Path,
) -> io::Result<(StorePathRef<'a>, &'a snix_castore::Path)> {
    let (store_path, rest) =
        StorePathRef::from_absolute_path_full(path).map_err(std::io::Error::other)?;

    use std::os::unix::ffi::OsStrExt;
    let sub_path = snix_castore::Path::from_bytes(rest.as_os_str().as_bytes())
        .ok_or_else(|| std::io::Error::other("sub_path is no valid path"))?;

    Ok((store_path, sub_path))
}

impl EvalIO for SnixStoreIO {
    #[instrument(skip(self), ret(level = Level::TRACE), err)]
    fn path_exists(&self, path: &std::path::Path) -> io::Result<bool> {
        if let Ok((store_path, sub_path)) = parse_store_and_sub_path(path) {
            if self
                .tokio_handle
                .block_on(self.store_path_to_path_info(&store_path, sub_path))?
                .is_some()
            {
                Ok(true)
            } else {
                // As snix-store doesn't manage /nix/store on the filesystem,
                // we still need to also ask self.std_io here.
                self.std_io.path_exists(path)
            }
        } else {
            // The store path is no store path, so do regular StdIO.
            self.std_io.path_exists(path)
        }
    }

    #[instrument(skip(self), err)]
    fn open(&self, path: &std::path::Path) -> io::Result<Box<dyn io::Read>> {
        if let Ok((store_path, sub_path)) = parse_store_and_sub_path(path) {
            self.tokio_handle.block_on(async {
                if let Some(path_info) = self.store_path_to_path_info(&store_path, sub_path).await?
                {
                    // depending on the node type, treat open differently
                    match path_info.node {
                        Node::Directory { .. } => {
                            // This would normally be a io::ErrorKind::IsADirectory (still unstable)
                            Err(io::Error::new(
                                io::ErrorKind::Unsupported,
                                format!("tried to open directory at {path:?}"),
                            ))
                        }
                        Node::File { digest, .. } => {
                            let t_blob = std::time::Instant::now();
                            let io_blob = snix_store::perf_stats::IO_WALL.enter();
                            let resp = self
                                .build_state
                                .blob_service
                                .as_ref()
                                .open_read(&digest)
                                .await?;
                            snix_store::perf_stats::BLOB_READ.record(t_blob);
                            drop(io_blob);
                            match resp {
                                Some(blob_reader) => {
                                    // The VM Response needs a sync [std::io::Reader].
                                    Ok(Box::new(SyncIoBridge::new(blob_reader))
                                        as Box<dyn io::Read>)
                                }
                                None => {
                                    error!(
                                        blob.digest = %digest,
                                        "blob not found",
                                    );
                                    Err(io::Error::new(
                                        io::ErrorKind::NotFound,
                                        format!("blob {} not found", &digest),
                                    ))
                                }
                            }
                        }
                        Node::Symlink { .. } => Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "open for symlinks is unsupported",
                        ))?,
                    }
                } else {
                    // As snix-store doesn't manage /nix/store on the filesystem,
                    // we still need to also ask self.std_io here.
                    self.std_io.open(path)
                }
            })
        } else {
            // The store path is no store path, so do regular StdIO.
            self.std_io.open(path)
        }
    }

    #[instrument(skip(self), ret(level = Level::TRACE), err)]
    fn file_type(&self, path: &std::path::Path) -> io::Result<FileType> {
        if let Ok((store_path, sub_path)) = parse_store_and_sub_path(path) {
            if let Some(path_info) = self
                .tokio_handle
                .block_on(async { self.store_path_to_path_info(&store_path, sub_path).await })?
            {
                Ok(node_get_type(&path_info.node))
            } else {
                self.std_io.file_type(path)
            }
        } else {
            self.std_io.file_type(path)
        }
    }

    #[instrument(skip(self), ret(level = Level::TRACE), err)]
    fn read_dir(&self, path: &std::path::Path) -> io::Result<Vec<(bytes::Bytes, FileType)>> {
        if let Ok((store_path, sub_path)) = parse_store_and_sub_path(path) {
            self.tokio_handle.block_on(async {
                if let Some(path_info) = self.store_path_to_path_info(&store_path, sub_path).await?
                {
                    match path_info.node {
                        Node::Directory { digest, .. } => {
                            // fetch the Directory itself.
                            let t_dir = std::time::Instant::now();
                            let io_dir = snix_store::perf_stats::IO_WALL.enter();
                            let directory = self
                                .build_state
                                .directory_service
                                .as_ref()
                                .get(&digest)
                                .await
                                .map_err(std::io::Error::other)?
                                .ok_or_else(|| {
                                    // If we didn't get the directory node that's linked, that's a store inconsistency!
                                    error!(
                                        directory.digest = %digest,
                                        path = ?path,
                                        "directory not found",
                                    );
                                    io::Error::new(
                                        io::ErrorKind::NotFound,
                                        format!("directory {digest} does not exist"),
                                    )
                                })?;
                            snix_store::perf_stats::DIR_GET.record(t_dir);
                            drop(io_dir);

                            // construct children from nodes
                            Ok(directory
                                .into_nodes()
                                .map(|(name, node)| (name.into(), node_get_type(&node)))
                                .collect())
                        }
                        Node::File { .. } => {
                            // This would normally be a io::ErrorKind::NotADirectory (still unstable)
                            Err(io::Error::new(
                                io::ErrorKind::Unsupported,
                                "tried to readdir path {:?}, which is a file",
                            ))?
                        }
                        Node::Symlink { .. } => Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "read_dir for symlinks is unsupported",
                        ))?,
                    }
                } else {
                    self.std_io.read_dir(path)
                }
            })
        } else {
            self.std_io.read_dir(path)
        }
    }

    #[instrument(skip(self), ret(level = Level::TRACE), err)]
    fn import_path(&self, path: &std::path::Path) -> io::Result<std::path::PathBuf> {
        let file_name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidFilename,
                "path without basename encountered",
            )
        })?;
        let name = nix_compat::store_path::validate_name_from_os_str(file_name)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            .to_owned();

        let path_info = self.tokio_handle.block_on(async {
            // If `path` already resolves inside the store (its tree lives in castore, e.g. a
            // subpath of nixpkgs during a full-snix eval where nothing is materialized on the
            // real filesystem), re-key that castore node into a NAR-CA path instead of walking
            // the real filesystem — which wouldn't have the tree.
            if let Ok((store_path, sub_path)) = parse_store_and_sub_path(path)
                && let Some(pi) = self.store_path_to_path_info(&store_path, sub_path).await?
            {
                return self.node_to_nar_ca_path_info(&name, pi.node).await;
            }
            snix_store::import::import_path_as_nar_ca(
                path,
                &name,
                &self.build_state.blob_service,
                &self.build_state.directory_service,
                &self.build_state.path_info_service,
                &self.build_state.nar_calculation_service,
            )
            .await
        })?;

        // From the returned PathInfo, extract the store path and return it.
        Ok(path_info.store_path.to_absolute_path().into())
    }

    #[instrument(skip(self), ret(level = Level::TRACE))]
    fn store_dir(&self) -> Option<String> {
        Some("/nix/store".to_string())
    }

    fn get_env(&self, key: &OsStr) -> Option<OsString> {
        env::var_os(key)
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, rc::Rc, sync::Arc};

    use bstr::ByteSlice;
    use clap::Parser;
    use snix_build::buildservice::DummyBuildService;
    use snix_eval::{EvalIO, EvaluationResult};
    use snix_store::utils::{ServiceUrlsMemory, construct_services};
    use tempfile::TempDir;

    use super::SnixStoreIO;
    use crate::builtins::{add_derivation_builtins, add_fetcher_builtins, add_import_builtins};

    /// evaluates a given nix expression and returns the result.
    /// Takes care of setting up the evaluator so it knows about the
    // `derivation` builtin.
    fn eval(str: &str) -> EvaluationResult {
        let tokio_runtime = tokio::runtime::Runtime::new().unwrap();
        let (blob_service, directory_service, path_info_service, nar_calculation_service) =
            tokio_runtime
                .block_on(async {
                    construct_services(ServiceUrlsMemory::parse_from(std::iter::empty::<&str>()))
                        .await
                })
                .unwrap();

        let io = Rc::new(SnixStoreIO::new(
            blob_service,
            directory_service,
            path_info_service,
            nar_calculation_service.into(),
            Arc::<DummyBuildService>::default(),
            tokio_runtime.handle().clone(),
            Vec::new(),
        ));

        let mut eval_builder =
            snix_eval::Evaluation::builder(io.clone() as Rc<dyn EvalIO>).enable_import();
        eval_builder = add_derivation_builtins(eval_builder, Rc::clone(&io));
        eval_builder = add_fetcher_builtins(eval_builder, Rc::clone(&io));
        eval_builder = add_import_builtins(eval_builder, io);
        let eval = eval_builder.build();

        // run the evaluation itself.
        eval.evaluate(str, None)
    }

    /// Helper function that takes a &Path, and invokes a snix evaluator coercing that path to a string
    /// (via "${/this/path}"). The path can be both absolute or not.
    /// It returns Option<String>, depending on whether the evaluation succeeded or not.
    fn import_path_and_compare<P: AsRef<Path>>(p: P) -> Option<String> {
        // Try to import the path using "${/tmp/path/to/test}".
        // The format string looks funny, the {} passed to Nix needs to be
        // escaped.
        let code = format!(r#""${{{}}}""#, p.as_ref().display());
        let result = eval(&code);

        if !result.errors.is_empty() {
            return None;
        }

        let value = result.value.expect("must be some");
        match value {
            snix_eval::Value::String(s) => Some(s.to_str_lossy().into_owned()),
            _ => panic!("unexpected value type: {value:?}"),
        }
    }

    /// Import a directory with a zero-sized ".keep" regular file.
    /// Ensure it matches the (pre-recorded) store path that Nix would produce.
    #[test]
    fn import_directory() {
        let tmpdir = TempDir::new().unwrap();

        // create a directory named "test"
        let src_path = tmpdir.path().join("test");
        std::fs::create_dir(&src_path).unwrap();

        // write a regular file `.keep`.
        std::fs::write(src_path.join(".keep"), vec![]).unwrap();

        // importing the path with .../test at the end.
        assert_eq!(
            Some("/nix/store/gq3xcv4xrj4yr64dflyr38acbibv3rm9-test".to_string()),
            import_path_and_compare(&src_path)
        );

        // importing the path with .../test/. at the end.
        assert_eq!(
            Some("/nix/store/gq3xcv4xrj4yr64dflyr38acbibv3rm9-test".to_string()),
            import_path_and_compare(src_path.join("."))
        );
    }

    /// Import a file into the store. Nix uses the "recursive"/NAR-based hashing
    /// scheme for these.
    #[test]
    fn import_file() {
        let tmpdir = TempDir::new().unwrap();

        // write a regular file `empty`.
        std::fs::write(tmpdir.path().join("empty"), vec![]).unwrap();

        assert_eq!(
            Some("/nix/store/lx5i78a4izwk2qj1nq8rdc07y8zrwy90-empty".to_string()),
            import_path_and_compare(tmpdir.path().join("empty"))
        );

        // write a regular file `hello.txt`.
        std::fs::write(tmpdir.path().join("hello.txt"), b"Hello World!").unwrap();

        assert_eq!(
            Some("/nix/store/925f1jb1ajrypjbyq7rylwryqwizvhp0-hello.txt".to_string()),
            import_path_and_compare(tmpdir.path().join("hello.txt"))
        );
    }

    /// Invoke toString on a nonexisting file, and access the .file attribute.
    /// This should not cause an error, because it shouldn't trigger an import,
    /// and leave the path as-is.
    #[test]
    fn nonexisting_path_without_import() {
        let result = eval("toString ({ line = 42; col = 42; file = /deep/thought; }.file)");

        assert!(result.errors.is_empty(), "expect evaluation to succeed");
        let value = result.value.expect("must be some");

        match value {
            snix_eval::Value::String(s) => {
                assert_eq!(*s, "/deep/thought");
            }
            _ => panic!("unexpected value type: {value:?}"),
        }
    }
}
