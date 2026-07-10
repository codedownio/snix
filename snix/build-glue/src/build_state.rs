use std::{collections::BTreeMap, io, sync::Arc};

use futures::{StreamExt, TryStreamExt as _};
use nix_compat::{
    nixhash::CAHash,
    store_path::{StorePath, StorePathRef},
};
use parking_lot::RwLock;
use snix_build::buildservice::{BuildService, BuildUpdate};
use snix_castore::{
    blob_engine::BlobEngine,
    directoryservice::{DirectoryService, traversal::descend_to},
};
use snix_store::{
    nar::NarCalculationService, path_info::PathInfo, pathinfoservice::PathInfoService,
};
use tracing::{Level, Span, instrument, warn};
use tracing_indicatif::span_ext::IndicatifSpanExt;
use url::Url;

use crate::{builder, fetchers::Fetcher, known_paths::KnownPaths};

pub struct BuildState {
    // This is public so helper functions can interact with the stores directly.
    pub blob_engine: Arc<dyn BlobEngine>,
    pub directory_service: Arc<dyn DirectoryService>,
    pub path_info_service: Arc<dyn PathInfoService>,
    pub nar_calculation_service: Arc<dyn NarCalculationService>,

    build_service: Arc<dyn BuildService>,

    #[allow(clippy::type_complexity)]
    pub fetcher: Fetcher<
        Arc<dyn BlobEngine>,
        Arc<dyn DirectoryService>,
        Arc<dyn PathInfoService>,
        Arc<dyn NarCalculationService>,
    >,

    // Paths known how to produce, by building or fetching.
    pub known_paths: RwLock<KnownPaths>,
}

impl BuildState {
    pub fn new(
        blob_engine: Arc<dyn BlobEngine>,
        directory_service: Arc<dyn DirectoryService>,
        path_info_service: Arc<dyn PathInfoService>,
        nar_calculation_service: Arc<dyn NarCalculationService>,
        build_service: Arc<dyn BuildService>,
        hashed_mirrors: Vec<Url>,
    ) -> Self {
        Self {
            blob_engine: blob_engine.clone(),
            directory_service: directory_service.clone(),
            path_info_service: path_info_service.clone(),
            nar_calculation_service: nar_calculation_service.clone(),
            build_service,
            fetcher: Fetcher::new(
                blob_engine,
                directory_service,
                path_info_service,
                nar_calculation_service,
                hashed_mirrors,
            ),
            known_paths: Default::default(),
        }
    }

    /// for a given [StorePath] and additional [snix_castore::Path] inside the store path,
    /// look up the [PathInfo], and if it exists, and then uses
    /// [descend_to] to return the [snix_castore::Node] specified by `sub_path`.
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
    #[instrument(skip(self, store_path), fields(store_path=%store_path, indicatif.pb_show=tracing::field::Empty), ret(level = Level::TRACE), err(level = Level::TRACE))]
    pub async fn store_path_to_path_info(
        &self,
        store_path: &StorePathRef<'_>,
        sub_path: &snix_castore::Path,
    ) -> io::Result<Option<PathInfo>> {
        Span::current().pb_set_message(&format!("Accessing {store_path}/{sub_path}"));

        // Find the root node for the store_path.
        // It asks the PathInfoService first, but in case there was a Derivation
        // produced that would build it, fall back to triggering the build.
        // To populate the input nodes, it might recursively trigger builds of
        // its dependencies too.
        let t_pi = std::time::Instant::now();
        let io_pi = (
            snix_store::perf_stats::IO_WALL.enter(),
            snix_store::perf_stats::PATHINFO_WALL.enter(),
        );
        let looked_up = self
            .path_info_service
            .as_ref()
            .get(*store_path.digest())
            .await
            .map_err(std::io::Error::other)?;
        snix_store::perf_stats::PATHINFO_GET.record(t_pi);
        drop(io_pi);
        let mut path_info = if let Some(path_info) = looked_up {
            path_info
        } else {
            // If there's no PathInfo found, this normally means we have to
            // trigger the build (and insert into PathInfoService, after
            // reference scanning).
            // However, as Snix is (currently) not managing /nix/store itself,
            // we return Ok(None) to let std_io take over.
            // While reading from store paths that are not known to Snix during
            // that evaluation clearly is an impurity, we still need to support
            // it for things like <nixpkgs> pointing to a store path.
            // In the future, these things will (need to) have PathInfo.
            //
            // The store path doesn't exist yet, so we need to fetch or build it.
            // We check for fetches first, as we might have both native
            // fetchers and FODs in KnownPaths, and prefer the former.
            // This will also find [Fetch] synthesized from
            // `builtin:fetchurl` Derivations.
            let maybe_fetch = self
                .known_paths
                .read()
                .get_fetch_for_output_path(store_path);
            if let Some((name, fetch)) = maybe_fetch {
                let t_fetch = std::time::Instant::now();
                let io_fetch = (
                    snix_store::perf_stats::IO_WALL.enter(),
                    snix_store::perf_stats::FETCH_WALL.enter(),
                );
                let (sp, path_info) = self
                    .fetcher
                    .ingest_and_persist(&name, fetch)
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                snix_store::perf_stats::FETCH.record(t_fetch);
                drop(io_fetch);

                debug_assert_eq!(
                    sp.to_absolute_path(),
                    store_path.to_absolute_path(),
                    "store path returned from fetcher must match store path we have in fetchers"
                );

                path_info
            } else {
                // Look up the derivation for this output path.
                let (drv_path, drv) = {
                    let known_paths = self.known_paths.read();
                    if let Some(drv_path) = known_paths.get_drv_path_for_output_path(store_path) {
                        (
                            drv_path.to_owned(),
                            known_paths
                                .get_drv_by_drvpath(&drv_path.as_ref())
                                .unwrap()
                                .to_owned(),
                        )
                    } else {
                        warn!(store_path=%store_path, "no drv found");
                        // let StdIO take over
                        return Ok(None);
                    }
                };
                let span = Span::current();
                span.pb_start();
                span.pb_set_style(&snix_tracing::PB_SPINNER_STYLE);
                span.pb_set_message(&format!("⏳Waiting for inputs {}", store_path));

                // derivation_to_build_request needs castore nodes for all inputs.
                // Provide them, which means, here is where we recursively build
                // all dependencies.
                let resolved_inputs = {
                    let known_paths = &self.known_paths.read();
                    builder::get_all_inputs(&drv, known_paths, |path| {
                        Box::pin(async move {
                            self.store_path_to_path_info(&path.as_ref(), snix_castore::Path::ROOT)
                                .await
                        })
                    })
                }
                .try_collect()
                .await?;

                // precompute the `ca` field in the PathInfo, so drv can be sent away owned.
                let mut ca = drv.fod_digest().map(|fod_digest| {
                    CAHash::Nar(nix_compat::nixhash::NixHash::Sha256(fod_digest.into()))
                });

                // synthesize the build request.
                let build_request = builder::derivation_into_build_request(drv, &resolved_inputs)?;

                // Assemble a list of all outputs, as well as a mapping table from output path to
                // possible reference paths.
                // We need this to understand the response later.
                let output_paths: Vec<StorePath> = build_request
                    .outputs
                    .iter()
                    .map(|o| {
                        StorePath::from_bytes(
                            o.path
                                .strip_prefix(&nix_compat::store_path::STORE_DIR_WITH_SLASH[1..])
                                .expect("output doesn't have expected store_dir prefix")
                                .as_bytes(),
                        )
                        .expect("Snix bug: cannot parse output as StorePath")
                    })
                    .collect();
                let refs_per_path: BTreeMap<&StorePath, Vec<&StorePath>> = output_paths
                    .iter()
                    .map(|o| {
                        (
                            o,
                            output_paths.iter().chain(resolved_inputs.keys()).collect(),
                        )
                    })
                    .collect();

                span.pb_set_message(&format!("🔨Building {}", store_path));

                // Create a build, await its completion and return a list of output data.
                struct ProducedOutput<'s> {
                    output_path: StorePathRef<'s>,
                    node: snix_castore::Node,
                    references: Vec<StorePath>,
                }

                // When driven by nox (NOX_SNIX_BUILD_LOG set), print per-derivation
                // sentinel lines the nox builder parses into build-progress activities.
                let nox_build_log = std::env::var_os("NOX_SNIX_BUILD_LOG").is_some();
                if nox_build_log {
                    eprintln!("NOX_BUILD_START /nix/store/{drv_path}");
                }

                let t_build = std::time::Instant::now();
                let io_build = (
                    snix_store::perf_stats::IO_WALL.enter(),
                    snix_store::perf_stats::BUILD_WALL.enter(),
                );
                // Tail of the build transcript, carried in the error on failure: callers
                // without a tracing subscriber otherwise only see "nonzero exit".
                let mut transcript_tail = TranscriptTail::default();
                let produced_outputs: Vec<ProducedOutput> = {
                    let mut produced_outputs: Vec<ProducedOutput> =
                        Vec::with_capacity(output_paths.len());
                    let mut build_updates = self.build_service.do_build(build_request);

                    while let Some(build_update) = build_updates.next().await {
                        match build_update {
                            BuildUpdate::ProducedOutput {
                                node,
                                idx,
                                refscan_needles,
                            } => {
                                let output_path = usize::try_from(idx)
                                    .ok()
                                    .and_then(|i| output_paths.get(i))
                                    .ok_or_else(|| std::io::Error::other("invalid idx"))?;

                                // ensure we only receive each ProducedOutput once.
                                if produced_outputs
                                    .iter()
                                    .any(|e| e.output_path == output_path.as_ref())
                                {
                                    return Err(std::io::Error::other(
                                        "received output multiple times",
                                    ));
                                }

                                produced_outputs.push(ProducedOutput {
                                    node,
                                    output_path: output_path.as_ref(),
                                    references: {
                                        let mut references =
                                            Vec::with_capacity(refscan_needles.len());

                                        // Map each output needle index back into a store path.
                                        for needle_idx in refscan_needles {
                                            let output = *refs_per_path
                                                .get(output_path)
                                                .ok_or(std::io::Error::other("invalid output"))?
                                                .get(needle_idx as usize)
                                                .ok_or(std::io::Error::other(
                                                    "invalid needle_idx",
                                                ))?;
                                            references.push(output.to_owned());
                                        }

                                        // Produce references sorted by name for consistency with nix narinfos
                                        references.sort();
                                        references
                                    },
                                })
                            }
                            BuildUpdate::ProducedStdout(chunk) => transcript_tail.push(&chunk),
                            BuildUpdate::ProducedStderr(chunk) => transcript_tail.push(&chunk),
                            BuildUpdate::BuildFailure(build_failure) => {
                                warn!(%build_failure, transcript_tail = %transcript_tail, "build failed");
                                Err(std::io::Error::other(format!(
                                    "{build_failure}; transcript tail: {transcript_tail}"
                                )))?
                            }
                        }
                    }

                    // ensure all outputs have been produced.
                    if produced_outputs.len() != output_paths.len() {
                        return Err(std::io::Error::other("did not receive all outputs"));
                    }

                    produced_outputs
                };
                snix_store::perf_stats::BUILD.record(t_build);
                drop(io_build);
                if nox_build_log {
                    eprintln!(
                        "NOX_BUILD_DONE /nix/store/{drv_path} {:.1}",
                        t_build.elapsed().as_secs_f64()
                    );
                }
                // FUTUREWORK: more validation?

                let mut out_path_info: Option<PathInfo> = None;

                // For each produced output, insert a PathInfo.
                // FUTUREWORK: check references for cycles and upload in the correct order.
                for ProducedOutput {
                    output_path,
                    node,
                    references,
                } in produced_outputs
                {
                    // calculate the nar representation
                    let t_nar = std::time::Instant::now();
                    let io_nar = (
                        snix_store::perf_stats::IO_WALL.enter(),
                        snix_store::perf_stats::NAR_WALL.enter(),
                    );
                    let (nar_size, nar_sha256) = self
                        .nar_calculation_service
                        .calculate_nar(&node)
                        .await
                        .map_err(std::io::Error::other)?;
                    snix_store::perf_stats::NAR_CALC.record(t_nar);
                    drop(io_nar);

                    // assemble the PathInfo to persist
                    let path_info = PathInfo {
                        store_path: output_path.to_owned(),
                        node,
                        references,
                        nar_size,
                        nar_sha256,
                        signatures: vec![],
                        deriver: Some(StorePath::from_name_and_digest_fixed(
                            drv_path
                                .name()
                                .strip_drv_ext()
                                .expect("missing .drv suffix"),
                            *drv_path.digest(),
                        )),
                        // CA derivations only have one output, so this only runs once.
                        ca: ca.take(),
                    };

                    self.path_info_service
                        .put(path_info.clone())
                        .await
                        .map_err(std::io::Error::other)?;

                    if *store_path == output_path {
                        out_path_info = Some(path_info);
                    }
                }

                out_path_info.ok_or(io::Error::other("build didn't produce store path"))?
            }
        };

        // now with the root_node and sub_path, descend to the node requested.
        let t_descend = std::time::Instant::now();
        let io_descend = (
            snix_store::perf_stats::IO_WALL.enter(),
            snix_store::perf_stats::DESCEND_WALL.enter(),
        );
        let node = descend_to(&self.directory_service, path_info.node.clone(), sub_path)
            .await
            .map_err(std::io::Error::other)?;
        snix_store::perf_stats::DESCEND.record(t_descend);
        drop(io_descend);
        Ok(node.map(|node| {
            path_info.node = node;
            path_info
        }))
    }
}

/// Keeps the last few KiB of a build's stdout/stderr, for error messages.
#[derive(Default)]
struct TranscriptTail(std::collections::VecDeque<u8>);

impl TranscriptTail {
    const CAPACITY: usize = 4096;

    fn push(&mut self, chunk: &[u8]) {
        self.0.extend(chunk.iter().copied());
        while self.0.len() > Self::CAPACITY {
            self.0.pop_front();
        }
    }
}

impl std::fmt::Display for TranscriptTail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (a, b) = self.0.as_slices();
        f.write_str(&String::from_utf8_lossy(a))?;
        f.write_str(&String::from_utf8_lossy(b))
    }
}
