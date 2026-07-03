//! Experiment: build a Nix `.drv` (or, with `--recursive`, a whole missing `.drv` graph) via
//! snix's build service, writing outputs to castore.
//!
//! Reads `.drv`s (aterm) from the local `/nix/store`, resolves input closures from the castore,
//! synthesizes snix BuildRequests, runs the sandboxed builds, reference-scans the outputs and
//! persists their PathInfo. This mirrors the build path in `snix_glue::snix_store_io`, but driven
//! directly from `.drv`s produced by CppNix eval instead of from a snix evaluation. Store/build
//! addrs come from env (BLOB_SERVICE_ADDR / DIRECTORY_SERVICE_ADDR / PATH_INFO_SERVICE_ADDR /
//! BUILD_SERVICE_ADDR).
//!
//! `--recursive` walks input_derivations from the top drv: subtrees whose outputs already have
//! PathInfo in castore are pruned (so a store fronted by a substituter — e.g. the
//! `cache:?near=&far=nix+https://…` composition — substitutes instead of building), and whatever
//! remains is built leaves-first. Input sources must already be in castore.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use futures::stream::{StreamExt, TryStreamExt};
use nix_compat::derivation::Derivation;
use nix_compat::nixhash::{CAHash, NixHash};
use nix_compat::store_path::{STORE_DIR, StorePath};
use snix_build::buildservice::{self, BuildService};
use snix_castore::Node;
use snix_castore::blobservice::BlobService;
use snix_castore::directoryservice::DirectoryService;
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

    /// Recursively build every input derivation whose outputs are missing from castore
    /// (leaves first), instead of requiring the full input closure to be present.
    #[arg(long)]
    recursive: bool,
}

fn read_drv(store_path: &StorePath<String>) -> Derivation {
    let p = format!("/nix/store/{}", store_path);
    let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("read drv {p}: {e}"));
    Derivation::from_aterm_bytes(&bytes).unwrap_or_else(|e| panic!("parse drv {p}: {e:?}"))
}

struct Services {
    blob_service: Arc<dyn BlobService>,
    directory_service: Arc<dyn DirectoryService>,
    path_info_service: Arc<dyn PathInfoService>,
    build_service: Box<dyn BuildService>,
}

/// Which of the given output paths have no PathInfo in castore.
/// A `get` (not `has`) on purpose: through a Cache{near,far} composition it substitutes.
async fn missing_paths(
    paths: Vec<StorePath<String>>,
    path_info_service: &Arc<dyn PathInfoService>,
) -> Result<Vec<StorePath<String>>, Box<dyn std::error::Error + Send + Sync>> {
    let missing: Vec<StorePath<String>> = futures::stream::iter(paths)
        .map(|sp| {
            let pis = path_info_service.clone();
            async move {
                Ok::<_, snix_store::pathinfoservice::Error>(
                    pis.get(*sp.digest()).await?.is_none().then_some(sp),
                )
            }
        })
        .buffer_unordered(8)
        .try_collect::<Vec<Option<StorePath<String>>>>()
        .await?
        .into_iter()
        .flatten()
        .collect();
    Ok(missing)
}

/// Is `path`'s full reference closure present in castore? Castore PathInfo presence does NOT
/// imply the closure invariant CppNix's "valid" gives (nox stores runtime closures; copies may
/// exclude to-be-built outputs), so a present output can still have dangling references.
/// `complete` memoizes paths whose closure was verified.
async fn closure_complete(
    path: &StorePath<String>,
    path_info_service: &Arc<dyn PathInfoService>,
    complete: &mut HashSet<StorePath<String>>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if complete.contains(path) {
        return Ok(true);
    }
    let mut visited: HashSet<StorePath<String>> = HashSet::new();
    let mut frontier: Vec<StorePath<String>> = vec![path.clone()];
    while !frontier.is_empty() {
        let to_fetch: Vec<StorePath<String>> = frontier
            .into_iter()
            .filter(|sp| !complete.contains(sp) && visited.insert(sp.clone()))
            .collect();
        let infos: Vec<(StorePath<String>, Option<PathInfo>)> =
            futures::stream::iter(to_fetch.into_iter())
                .map(|sp| {
                    let pis = path_info_service.clone();
                    async move {
                        let info = pis.get(*sp.digest()).await.map_err(std::io::Error::other)?;
                        Ok::<_, std::io::Error>((sp, info))
                    }
                })
                .buffer_unordered(64)
                .try_collect()
                .await?;
        let mut next = Vec::new();
        for (sp, info) in infos {
            let Some(info) = info else {
                return Ok(false); // dangling reference somewhere below `path`
            };
            for r in &info.references {
                if *r != sp && !visited.contains(r) {
                    next.push(r.clone());
                }
            }
        }
        frontier = next;
    }
    complete.extend(visited);
    Ok(true)
}

/// Walk input_derivations from `top`, pruning subtrees whose NEEDED outputs are already in castore
/// WITH a complete reference closure (substituted or previously built). Only the outputs a parent
/// actually references are checked — a substitutable drv with some other uncached output must not
/// drag its build closure in. A drv whose outputs are present but whose closure dangles is
/// traversed (not rebuilt) so the drvs producing the dangling paths get planned.
/// Returns the drvs to build, leaves first.
async fn plan_missing(
    top: &StorePath<String>,
    path_info_service: &Arc<dyn PathInfoService>,
) -> Result<Vec<StorePath<String>>, Box<dyn std::error::Error + Send + Sync>> {
    // drv -> (needs building, its input drvs); recorded for built AND traversed drvs so topo
    // dependencies propagate through present-but-dangling intermediates.
    let mut graph: HashMap<StorePath<String>, (bool, Vec<StorePath<String>>)> = HashMap::new();
    // (drv, output name) pairs already checked (or scheduled on the frontier)
    let mut checked: HashSet<(StorePath<String>, String)> = HashSet::new();
    let mut complete: HashSet<StorePath<String>> = HashSet::new();

    let top_drv = read_drv(top);
    let top_outs: Vec<String> = top_drv.outputs.keys().cloned().collect();
    let mut frontier: VecDeque<(StorePath<String>, Vec<String>)> =
        VecDeque::from([(top.clone(), top_outs)]);
    while let Some((d, outs)) = frontier.pop_front() {
        let outs: Vec<String> = outs
            .into_iter()
            .filter(|o| checked.insert((d.clone(), o.clone())))
            .collect();
        if outs.is_empty() {
            continue;
        }
        let drv = read_drv(&d);
        let out_paths: Vec<StorePath<String>> = outs
            .iter()
            .map(|o| {
                drv.outputs
                    .get(o)
                    .unwrap_or_else(|| panic!("drv {d} has no output {o}"))
                    .path
                    .clone()
                    .expect("drv output has no store path")
            })
            .collect();
        let missing = missing_paths(out_paths.clone(), path_info_service).await?;
        let mut build = !missing.is_empty();
        if !build {
            let mut dangling = false;
            for p in &out_paths {
                if !closure_complete(p, path_info_service, &mut complete).await? {
                    dangling = true;
                    break;
                }
            }
            if !dangling {
                continue; // needed outputs present with complete closures: prune
            }
            build = false; // present but dangling: traverse children, don't rebuild
        }
        match graph.get_mut(&d) {
            Some((b, _)) => {
                *b |= build; // already traversed; maybe upgrade to build
                continue;
            }
            None => {}
        }
        let children: Vec<StorePath<String>> =
            drv.input_derivations.keys().cloned().collect();
        for (c, couts) in &drv.input_derivations {
            frontier.push_back((c.clone(), couts.iter().cloned().collect()));
        }
        graph.insert(d, (build, children));
    }

    // Topo-sort the recorded drvs (children before parents), then keep only the ones to build.
    let mut order: Vec<StorePath<String>> = Vec::with_capacity(graph.len());
    let mut done: HashSet<StorePath<String>> = HashSet::new();
    while order.len() < graph.len() {
        let mut progressed = false;
        let mut ready: Vec<StorePath<String>> = graph
            .iter()
            .filter(|(d, (_, deps))| {
                !done.contains(*d)
                    && deps.iter().all(|c| !graph.contains_key(c) || done.contains(c))
            })
            .map(|(d, _)| d.clone())
            .collect();
        ready.sort();
        for d in ready {
            done.insert(d.clone());
            order.push(d);
            progressed = true;
        }
        if !progressed {
            return Err("dependency cycle among missing drvs".into());
        }
    }
    Ok(order
        .into_iter()
        .filter(|d| graph[d].0)
        .collect())
}

/// The declared inputs of a drv: input_sources + each input derivation's requested output paths
/// (read from the input .drv itself, since we have no eval-populated known_paths).
fn declared_input_seeds(drv: &Derivation) -> Vec<StorePath<String>> {
    let mut seeds: Vec<StorePath<String>> = drv.input_sources.iter().cloned().collect();
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
            seeds.push(p);
        }
    }
    seeds
}

/// Walk the reference closure of the seeds, resolving each to a castore Node from PathInfo.
/// Each PathInfo.get is a store round-trip and the closure is large (100s of paths), so we fetch
/// a whole BFS level concurrently rather than serially -- serial grpc otherwise dominates the
/// build wall-time (~10s for this env vs ~sub-second for the actual build).
/// Paths without PathInfo are returned as missing (their references can't be expanded);
/// `skip` paths are not resolved at all (used for outputs that a planned earlier build produces).
async fn resolve_input_closure(
    seeds: Vec<StorePath<String>>,
    skip: &HashSet<StorePath<String>>,
    services: &Services,
) -> Result<
    (BTreeMap<StorePath<String>, Node>, Vec<StorePath<String>>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let mut visited: HashSet<StorePath<String>> = HashSet::new();
    let mut resolved_inputs: BTreeMap<StorePath<String>, Node> = BTreeMap::new();
    let mut missing: Vec<StorePath<String>> = Vec::new();
    let mut frontier: Vec<StorePath<String>> = seeds;
    while !frontier.is_empty() {
        let to_fetch: Vec<StorePath<String>> = frontier
            .into_iter()
            .filter(|sp| !skip.contains(sp) && visited.insert(sp.clone()))
            .collect();
        let infos: Vec<(StorePath<String>, Option<PathInfo>)> =
            futures::stream::iter(to_fetch.into_iter())
                .map(|sp| {
                    let path_info_service = services.path_info_service.clone();
                    async move {
                        let info = path_info_service
                            .get(*sp.digest())
                            .await
                            .map_err(std::io::Error::other)?;
                        Ok::<_, std::io::Error>((sp, info))
                    }
                })
                .buffer_unordered(64)
                .try_collect()
                .await?;
        let mut next = Vec::new();
        for (sp, info) in infos {
            let Some(info) = info else {
                missing.push(sp);
                continue;
            };
            for r in &info.references {
                if !visited.contains(r) {
                    next.push(r.clone());
                }
            }
            resolved_inputs.insert(info.store_path, info.node);
        }
        frontier = next;
    }
    Ok((resolved_inputs, missing))
}

/// Build one drv whose full input closure is present in castore; persist output PathInfo.
async fn build_one(
    drv_path: &StorePath<String>,
    services: &Services,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let drv = read_drv(drv_path);

    // Compute output NARs locally from the blob+directory services, rather than via the grpc NAR
    // service that construct_services prefers when the PathInfo client advertises one: nox-store
    // (unlike snix's own daemon) does not implement remote NAR calculation.
    let nar_calculation_service = snix_store::nar::SimpleRenderer::new(
        services.blob_service.clone(),
        services.directory_service.clone(),
    );

    let seeds = declared_input_seeds(&drv);
    let (resolved_inputs, missing) =
        resolve_input_closure(seeds, &HashSet::new(), services).await?;
    if !missing.is_empty() {
        return Err(format!(
            "path_info missing in castore for {} paths (copy their closures first), e.g. {}",
            missing.len(),
            missing[0]
        )
        .into());
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
    let build_result = services
        .build_service
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
        services.path_info_service.put(path_info).await?;

        println!(
            "OUTPUT /nix/store/{}  ({} refs, nar_size={}, nar_sha256={})",
            output_path,
            references.len(),
            nar_size,
            data_encoding::HEXLOWER.encode(&nar_sha256)
        );
    }

    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    let (blob_service, directory_service, path_info_service, _nar_calculation_service) =
        construct_services(args.service_addrs).await?;
    let build_service = buildservice::from_addr(
        &args.build_service_addr,
        blob_service.clone(),
        directory_service.clone(),
    )
    .await?;
    let services = Services {
        blob_service,
        directory_service,
        path_info_service,
        build_service,
    };

    let drv_path = StorePath::<String>::from_bytes(
        Path::new(&args.drv).file_name().expect("drv path").as_bytes(),
    )?;

    if !args.recursive {
        return build_one(&drv_path, &services).await;
    }

    let plan = plan_missing(&drv_path, &services.path_info_service).await?;
    if plan.is_empty() {
        eprintln!("nothing to build: all outputs of {} present in castore", drv_path);
        return Ok(());
    }
    eprintln!("building {} missing drvs (leaves first):", plan.len());
    for d in &plan {
        eprintln!("  {d}");
    }

    // Pre-check every planned build's declared input closure and report ALL missing paths in one
    // shot (exit 3), so a driver can substitute/ingest them and retry, instead of failing one
    // missing path at a time. Outputs of planned drvs will exist once their build runs — skip them.
    let planned_outputs: HashSet<StorePath<String>> = plan
        .iter()
        .flat_map(|d| {
            read_drv(d)
                .outputs
                .values()
                .map(|o| o.path.clone().expect("drv output has no store path"))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut missing_all: std::collections::BTreeSet<StorePath<String>> = Default::default();
    for d in &plan {
        let seeds = declared_input_seeds(&read_drv(d));
        let (_, missing) = resolve_input_closure(seeds, &planned_outputs, &services).await?;
        missing_all.extend(missing);
    }
    if !missing_all.is_empty() {
        for p in &missing_all {
            println!("MISSING_INPUT /nix/store/{p}");
        }
        eprintln!(
            "{} input paths missing from castore; ingest them and re-run",
            missing_all.len()
        );
        std::process::exit(3);
    }

    let total = plan.len();
    for (i, d) in plan.iter().enumerate() {
        eprintln!("[{}/{}] {}", i + 1, total, d);
        let t = std::time::Instant::now();
        build_one(d, &services).await?;
        eprintln!("[{}/{}] {} done in {:.1}s", i + 1, total, d, t.elapsed().as_secs_f64());
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
