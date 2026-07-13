use clap::Parser;
use mimalloc::MiMalloc;
use nix_compat::nix_daemon::handler::NixDaemon;
use nix_daemon::SnixDaemon;
use snix_store::pathinfoservice::{CachePathInfoService, LruPathInfoService, PathInfoService};
use snix_store::utils::{ServiceUrlsGrpc, construct_services};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::{error::Error, num::NonZeroUsize, sync::Arc};
use tokio_listener::SystemOptions;
use tracing::{error, info};

/// nox: counts every pathinfo query the daemon serves and the time spent answering, so pod logs
/// show how much of a build's wall-clock goes to store metadata queries. Logged periodically.
struct CountingPathInfoService {
    inner: Arc<dyn PathInfoService>,
    gets: AtomicU64,
    found: AtomicU64,
    nanos: AtomicU64,
}

#[tonic::async_trait]
impl PathInfoService for CountingPathInfoService {
    async fn get(
        &self,
        digest: [u8; 20],
    ) -> Result<Option<snix_store::pathinfoservice::PathInfo>, snix_store::pathinfoservice::Error>
    {
        let t = std::time::Instant::now();
        let r = self.inner.get(digest).await;
        self.nanos.fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
        self.gets.fetch_add(1, Relaxed);
        if let Ok(Some(_)) = &r {
            self.found.fetch_add(1, Relaxed);
        }
        r
    }

    async fn put(
        &self,
        path_info: snix_store::pathinfoservice::PathInfo,
    ) -> Result<snix_store::pathinfoservice::PathInfo, snix_store::pathinfoservice::Error> {
        self.inner.put(path_info).await
    }

    fn list(
        &self,
    ) -> futures::stream::BoxStream<
        'static,
        Result<snix_store::pathinfoservice::PathInfo, snix_store::pathinfoservice::Error>,
    > {
        self.inner.list()
    }
}

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Run Nix-compatible store daemon backed by snix.
#[derive(Parser)]
struct Cli {
    #[clap(flatten)]
    service_addrs: ServiceUrlsGrpc,

    /// The address to listen on. Must be a unix domain socket.
    #[clap(flatten)]
    listen_args: tokio_listener::ListenerAddressLFlag,

    #[clap(flatten)]
    tracing_args: snix_tracing::TracingArgs,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args = Cli::parse();

    let mut tracing_handle = snix_tracing::TracingBuilder::default()
        .handle_tracing_args(&args.tracing_args)
        .build()?;

    tokio::select! {
        _ = snix_cli::shutdown_signal() => {
            if let Err(e) = tracing_handle.shutdown().await {
                eprintln!("failed to shutdown tracing: {e}");
            }
            Ok(())
        },
        res = run(args) => {
            if let Err(e) = tracing_handle.shutdown().await {
                eprintln!("failed to shutdown tracing: {e}");
            }
            res
        }
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (blob_service, directory_service, path_info_service, _nar_calculation_service) =
        construct_services(cli.service_addrs).await?;

    // nox: front the (remote) path-info service with a local LRU read-through cache. A build's Nix
    // eval issues tens of thousands of IsValidPath/QueryPathInfo/QueryValidPaths queries -- often for
    // the same store paths repeatedly, and a rebuild re-queries almost the same closure. Path info is
    // immutable, so caching it locally turns those per-query round-trips to the remote store
    // (nox-store -> Postgres) into local hits. Capacity via NIX_DAEMON_PATHINFO_CACHE_ENTRIES
    // (0 disables); default 100000.
    let path_info_service: Arc<dyn PathInfoService> = {
        let cap = std::env::var("NIX_DAEMON_PATHINFO_CACHE_ENTRIES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(100_000);
        match NonZeroUsize::new(cap) {
            None => path_info_service,
            Some(cap) => Arc::new(CachePathInfoService::new(
                "nox-pathinfo-cache".to_string(),
                LruPathInfoService::with_capacity("nox-pathinfo-lru".to_string(), cap),
                path_info_service,
            )),
        }
    };

    // Count queries + daemon-side answer time; log every 5s while queries are flowing.
    let counting = Arc::new(CountingPathInfoService {
        inner: path_info_service,
        gets: AtomicU64::new(0),
        found: AtomicU64::new(0),
        nanos: AtomicU64::new(0),
    });
    let path_info_service: Arc<dyn PathInfoService> = counting.clone();
    tokio::spawn({
        let c = counting;
        async move {
            let mut last = 0u64;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let gets = c.gets.load(Relaxed);
                if gets != last {
                    last = gets;
                    info!(
                        gets,
                        found = c.found.load(Relaxed),
                        total_ms = c.nanos.load(Relaxed) / 1_000_000,
                        "pathinfo query stats (cumulative)"
                    );
                }
            }
        }
    });

    let listen_address = cli.listen_args.listen_address.unwrap_or_else(|| {
        "/tmp/snix-daemon.sock"
            .parse()
            .expect("invalid fallback listen address")
    });

    let mut listener = tokio_listener::Listener::bind(
        &listen_address,
        &SystemOptions::default(),
        &cli.listen_args.listener_options,
    )
    .await?;

    let io = Arc::new(SnixDaemon::new(
        blob_service,
        directory_service,
        path_info_service,
    ));

    while let Ok((connection, _)) = listener.accept().await {
        let io = io.clone();
        tokio::spawn(async move {
            match NixDaemon::initialize(io.clone(), connection).await {
                Ok(mut daemon) => {
                    if let Err(error) = daemon.handle_client().await {
                        match error.kind() {
                            std::io::ErrorKind::UnexpectedEof => {
                                // client disconnected, nothing to do
                            }
                            _ => {
                                // otherwise log the error and disconnect
                                error!(error=?error, "client error");
                            }
                        }
                    }
                }
                Err(error) => {
                    error!(error=?error, "nix-daemon handshake failed");
                }
            }
        });
    }
    Ok(())
}
