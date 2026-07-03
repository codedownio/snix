//! Experiment: build a single Nix `.drv` via snix's build service, writing outputs to castore.
//!
//! Reads a `.drv` (aterm) from the local `/nix/store`, resolves its full input closure from the
//! castore (which must be pre-populated, e.g. via `snix-store copy`), synthesizes a snix
//! BuildRequest, runs the sandboxed build, reference-scans the outputs and persists their PathInfo.
//! This mirrors the build path in `snix_glue::snix_store_io`, but driven directly from a `.drv`
//! produced by CppNix eval instead of from a snix evaluation. Store/build addrs come from env
//! (BLOB_SERVICE_ADDR / DIRECTORY_SERVICE_ADDR / PATH_INFO_SERVICE_ADDR / BUILD_SERVICE_ADDR).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use clap::Parser;
use nix_compat::derivation::Derivation;
use nix_compat::nixhash::{CAHash, NixHash};
use nix_compat::store_path::{STORE_DIR, StorePath};
use snix_build::buildservice::{self, BuildService};
use snix_castore::Node;
use snix_glue::builder::derivation_into_build_request;
use snix_store::nar::NarCalculationService;
use snix_store::pathinfoservice::{PathInfo, PathInfoService};
use snix_store::utils::{ServiceUrlsMemory, construct_services};

#[derive(Parser)]
struct Args {
    /// Path to the top-level `.drv` to build (aterm file in the local /nix/store).
    drv: String,

    #[command(flatten)]
    service_addrs: ServiceUrlsMemory,

    #[arg(long, env = "BUILD_SERVICE_ADDR", default_value = "dummy:")]
    build_service_addr: String,
}

fn read_drv(store_path: &StorePath<String>) -> Derivation {
    let p = format!("/nix/store/{}", store_path);
    let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("read drv {p}: {e}"));
    Derivation::from_aterm_bytes(&bytes).unwrap_or_else(|e| panic!("parse drv {p}: {e:?}"))
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    let (blob_service, directory_service, path_info_service, _nar_calculation_service) =
        construct_services(args.service_addrs).await?;
    // Compute output NARs locally from the blob+directory services, rather than via the grpc NAR
    // service that construct_services prefers when the PathInfo client advertises one: nox-store
    // (unlike snix's own daemon) does not implement remote NAR calculation.
    let nar_calculation_service =
        snix_store::nar::SimpleRenderer::new(blob_service.clone(), directory_service.clone());
    let build_service = buildservice::from_addr(
        &args.build_service_addr,
        blob_service.clone(),
        directory_service.clone(),
    )
    .await?;

    // Parse the top .drv.
    let drv_path = StorePath::<String>::from_bytes(
        Path::new(&args.drv).file_name().expect("drv path").as_bytes(),
    )?;
    let drv = read_drv(&drv_path);

    // Seed the input queue with input_sources + each input derivation's requested output paths
    // (read from the input .drv itself, since we have no eval-populated known_paths).
    let mut queue: VecDeque<StorePath<String>> = VecDeque::new();
    for s in &drv.input_sources {
        queue.push_back(s.clone());
    }
    for (idrv_path, outs) in &drv.input_derivations {
        let idrv = read_drv(idrv_path);
        for out in outs {
            let p = idrv
                .outputs
                .get(out)
                .unwrap_or_else(|| panic!("input drv {idrv_path} has no output {out}"))
                .path
                .clone()
                .expect("input drv output has no store path");
            queue.push_back(p);
        }
    }

    // Walk the reference closure of those inputs, resolving each to a castore Node from PathInfo.
    // Each PathInfo.get is a store round-trip and the closure is large (100s of paths), so we fetch
    // a whole BFS level concurrently rather than serially -- serial grpc otherwise dominates the
    // build wall-time (~10s for this env vs ~sub-second for the actual build).
    use futures::stream::{StreamExt, TryStreamExt};
    let mut visited: HashSet<StorePath<String>> = HashSet::new();
    let mut resolved_inputs: BTreeMap<StorePath<String>, Node> = BTreeMap::new();
    let mut frontier: Vec<StorePath<String>> = queue.into_iter().collect();
    while !frontier.is_empty() {
        let to_fetch: Vec<StorePath<String>> = frontier
            .into_iter()
            .filter(|sp| visited.insert(sp.clone()))
            .collect();
        let infos: Vec<PathInfo> = futures::stream::iter(to_fetch.into_iter())
            .map(|sp| {
                let path_info_service = path_info_service.clone();
                async move {
                    path_info_service
                        .get(*sp.digest())
                        .await
                        .map_err(std::io::Error::other)?
                        .ok_or_else(|| {
                            std::io::Error::other(format!(
                                "path_info missing in castore for {sp} (copy its closure first)"
                            ))
                        })
                }
            })
            .buffer_unordered(64)
            .try_collect()
            .await?;
        let mut next = Vec::new();
        for info in infos {
            for r in &info.references {
                if !visited.contains(r) {
                    next.push(r.clone());
                }
            }
            resolved_inputs.insert(info.store_path, info.node);
        }
        frontier = next;
    }
    eprintln!("resolved {} input paths from castore", resolved_inputs.len());

    // Precompute the `ca` field (only set for fixed-output derivations).
    let mut ca = drv
        .fod_digest()
        .map(|fod_digest| CAHash::Nar(NixHash::Sha256(fod_digest)));

    let build_request = derivation_into_build_request(drv, &resolved_inputs)?;

    // Map refscan-needle indexes back to store paths: outputs first, then the input closure
    // (same order derivation_into_build_request assembles refscan_needles).
    let mut output_paths: Vec<StorePath<String>> = Vec::with_capacity(build_request.outputs.len());
    let all_possible_refs: Vec<StorePath<String>> = build_request
        .outputs
        .iter()
        .map(|p| {
            let sp = StorePath::<String>::from_bytes(
                p.strip_prefix(&STORE_DIR[1..])
                    .expect("output doesn't have expected store_dir prefix")
                    .as_os_str()
                    .as_bytes(),
            )
            .expect("cannot parse output as StorePath");
            output_paths.push(sp.clone());
            sp
        })
        .chain(resolved_inputs.keys().cloned())
        .collect();

    eprintln!("🔨 building {} ...", drv_path);
    let build_result = build_service
        .do_build(build_request)
        .await
        .map_err(|e| format!("do_build failed: {e}"))?;

    for (output, output_path) in build_result.outputs.into_iter().zip(output_paths) {
        let (nar_size, nar_sha256) = nar_calculation_service.calculate_nar(&output.node).await?;

        let mut references: Vec<StorePath<String>> = Vec::with_capacity(output.output_needles.len());
        for needle_idx in output.output_needles {
            references.push(
                all_possible_refs
                    .get(needle_idx as usize)
                    .ok_or("invalid needle_idx")?
                    .clone(),
            );
        }
        references.sort();

        let path_info = PathInfo {
            store_path: output_path.clone(),
            node: output.node,
            references: references.clone(),
            nar_size,
            nar_sha256,
            signatures: vec![],
            deriver: Some(
                StorePath::from_name_and_digest_fixed(
                    drv_path.name().strip_suffix(".drv").expect("missing .drv suffix"),
                    *drv_path.digest(),
                )
                .expect("deriver StorePath"),
            ),
            ca: ca.take(),
        };
        path_info_service.put(path_info).await?;

        println!(
            "OUTPUT /nix/store/{}  ({} refs, nar_size={})",
            output_path,
            references.len(),
            nar_size
        );
    }

    Ok(())
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    if let Err(e) = rt.block_on(run()) {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}
