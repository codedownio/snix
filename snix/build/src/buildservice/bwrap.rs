use std::path::{Path, PathBuf};

use futures::{StreamExt, stream::BoxStream};
use snix_castore::{
    blob_engine::BlobEngine, directoryservice::DirectoryService, fs::fuse::FuseDaemon,
};
use tracing::{Span, info, instrument, warn};
use uuid::Uuid;

use super::BuildService;
use crate::{
    buildservice::{BuildConstraints, BuildRequest, BuildUpdate, streaming::run_build_streaming},
    bwrap::Bwrap,
    sandbox::SandboxSpec,
};
const SANDBOX_SHELL: &str = env!("SNIX_BUILD_SANDBOX_SHELL");

pub struct BubblewrapBuildService<BE, DS> {
    /// Root path in which all builds run
    workdir: PathBuf,

    /// Handle to a [BlobEngine], used by filesystems spawned during builds.
    blob_engine: BE,
    /// Handle to a [DirectoryService], used by filesystems spawned during builds.
    directory_service: DS,

    /// Optional: an already-mounted whole-store directory to bind as the build's read-only inputs
    /// (e.g. a cached nox-mount FUSE) instead of spinning up a per-build castore FUSE from the
    /// request's inputs. When set, the sandbox reads inputs through this mount (and its cache).
    external_store: Option<PathBuf>,

    /// Whether the sandbox unshares a user namespace (see [SandboxSpec::userns]).
    userns: bool,

    // semaphore to track number of concurrently running builds.
    // this is necessary, as otherwise we very quickly run out of open file handles.
    concurrent_builds: tokio::sync::Semaphore,
}
impl<BS, DS> BubblewrapBuildService<BS, DS> {
    pub fn new(
        workdir: PathBuf,
        blob_engine: BS,
        directory_service: DS,
        external_store: Option<PathBuf>,
        userns: bool,
    ) -> Self {
        // We map root inside the container to the uid/gid this is running at,
        // and allocate one for uid 1000 into the container from the range we
        // got in /etc/sub{u,g}id.
        // FUTUREWORK: use different uids?
        Self {
            workdir,
            blob_engine,
            directory_service,
            external_store,
            userns,
            concurrent_builds: tokio::sync::Semaphore::new(2),
        }
    }
}

impl<BE, DS> BubblewrapBuildService<BE, DS>
where
    BE: BlobEngine + Clone + 'static,
    DS: DirectoryService + Clone + 'static,
{
    fn make_spec(&self, request: BuildRequest, sandbox_path: &Path) -> SandboxSpec {
        let blob_engine = self.blob_engine.clone();
        let directory_service = self.directory_service.clone();
        let external = self.external_store.is_some();

        // In external mode, bind only the declared inputs (the .drv's input closure) from the mount,
        // so the output path is never in the sandbox's store view. Keys are the store-path basenames.
        let external_input_names: Vec<std::path::PathBuf> = if external {
            use std::os::unix::ffi::OsStrExt;
            request
                .inputs
                .keys()
                .map(|k| std::path::PathBuf::from(std::ffi::OsStr::from_bytes(k.as_ref())))
                .collect()
        } else {
            Vec::new()
        };

        SandboxSpec::builder()
            .host_workdir(sandbox_path.to_path_buf())
            .sandbox_workdir(request.working_dir)
            .scratches(request.scratch_paths)
            .command(request.command_args)
            .env_vars(request.environment_vars)
            .additional_files(request.additional_files)
            // #2: when an external whole-store mount is configured, read inputs through it (bwrap
            // binds it) instead of a per-build castore FUSE.
            .external_inputs(self.external_store.clone())
            .external_input_names(external_input_names)
            .with_inputs(request.inputs_dir, move |path| -> std::io::Result<Box<dyn crate::sandbox::InputsGuard>> {
                if external {
                    // Inputs are served by the external mount (bound by bwrap); no FUSE to mount.
                    return Ok(Box::new(()));
                }
                let root_nodes = Box::new(request.inputs.clone());
                let fs = snix_castore::fs::SnixStoreFs::new(
                    blob_engine.clone(),
                    directory_service.clone(),
                    root_nodes,
                    snix_castore::fs::FSSettings {
                        list_root: true,
                        uid_gid_override: None,
                        show_xattr: false,
                    },
                    tokio::runtime::Handle::current(),
                );
                // FUTUREWORK: make fuse daemon threads configurable?
                Ok(Box::new(FuseDaemon::new(fs, path, 4, false)?))
            })
            .allow_network(
                request
                    .constraints
                    .contains(&BuildConstraints::NetworkAccess),
            )
            .userns(self.userns)
            .provide_shell(
                request
                    .constraints
                    .contains(&BuildConstraints::ProvideBinSh)
                    .then_some(SANDBOX_SHELL.into()),
            )
            .build()
    }
}

impl<BE, DS> BuildService for BubblewrapBuildService<BE, DS>
where
    BE: BlobEngine + Clone + 'static,
    DS: DirectoryService + Clone + 'static,
{
    #[instrument(skip_all, fields(build.name=tracing::field::Empty))]
    fn do_build(&self, request: BuildRequest) -> BoxStream<'_, BuildUpdate> {
        let span = Span::current();
        let build_name = Uuid::new_v4();
        let sandbox_path = self.workdir.join(build_name.to_string());
        span.record("build.name", build_name.to_string());

        let outputs = request.outputs.clone();

        let spec = self.make_spec(request, &sandbox_path);

        let updates = run_build_streaming(
            &self.concurrent_builds,
            self.blob_engine.clone(),
            self.directory_service.clone(),
            outputs,
            move || async move {
                info!("Starting bwrap build");

                let (stream, finder) = Bwrap::initialize(spec)?.run()?;
                Ok((stream, finder))
            },
        );

        persist_transcript(updates, sandbox_path)
    }
}

/// Always persist the sandbox transcript next to the sandbox, success or failure, as it streams
/// by: `<sandbox>/build-stdout.log` and `<sandbox>/build-stderr.log`. A 0-exit build can still
/// be silently wrong (e.g. stdenv phases no-op'ing), and this is the only record of what ran; a
/// driver (nox) stages these files as the build's log. Best-effort: a write failure is logged
/// once and never fails the build.
fn persist_transcript(
    updates: BoxStream<'_, BuildUpdate>,
    sandbox_path: PathBuf,
) -> BoxStream<'_, BuildUpdate> {
    use std::io::Write;

    let mut files: Option<(std::fs::File, std::fs::File)> = None;
    let mut dead = false;
    let open = move |name: &str| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(sandbox_path.join(name))
    };
    updates
        .inspect(move |update| {
            if dead {
                return;
            }
            let (data, which): (&[u8], usize) = match update {
                BuildUpdate::ProducedStdout(d) => (d.as_slice(), 0),
                BuildUpdate::ProducedStderr(d) => (d.as_slice(), 1),
                _ => return,
            };
            if files.is_none() {
                match (open("build-stdout.log"), open("build-stderr.log")) {
                    (Ok(out), Ok(err)) => files = Some((out, err)),
                    (Err(e), _) | (_, Err(e)) => {
                        warn!(error = %e, "cannot persist build transcript");
                        dead = true;
                        return;
                    }
                }
            }
            let (out, err) = files.as_mut().expect("opened above");
            let file = if which == 0 { out } else { err };
            if let Err(e) = file.write_all(data) {
                warn!(error = %e, "failed writing build transcript; no longer persisting");
                dead = true;
            }
        })
        .boxed()
}
