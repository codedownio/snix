use std::path::PathBuf;

use clap::{Parser, crate_version};
use snix_store::utils::ServiceUrlsMemory;
use url::Url;

fn make_version() -> String {
    #[allow(unused_mut)]
    let mut features = String::new();
    #[cfg(feature = "otlp")]
    {
        features.push_str("+otlp");
    }
    #[cfg(feature = "tracing-chrome")]
    {
        features.push_str("+tracing-chrome");
    }
    #[cfg(feature = "tracing-tracy")]
    {
        features.push_str("+tracing-tracy");
    }
    #[cfg(feature = "xp-store-composition-cli")]
    {
        features.push_str("+xp-store-composition-cli");
    }
    if features.is_empty() {
        String::from(crate_version!())
    } else {
        format!("{} ({features})", crate_version!())
    }
}

/// Provides a CLI interface to trigger evaluation using snix-eval.
///
/// Uses configured snix-{ca,}store and snix-build components,
/// and by default a set of builtins similar to these present in Nix.
///
/// None of the stores available add to the local `/nix/store` location.
///
/// The CLI interface is not stable and subject to change.
#[derive(Parser, Clone)]
#[command(version = make_version())]
pub struct Args {
    /// Path to a script to evaluate
    pub script: Option<PathBuf>,

    #[clap(long, short = 'E')]
    pub expr: Option<String>,

    /// Dump the raw AST to stdout before interpreting
    #[clap(long, env = "SNIX_DISPLAY_AST")]
    pub display_ast: bool,

    /// Dump the bytecode to stdout before evaluating
    #[clap(long, env = "SNIX_DUMP_BYTECODE")]
    pub dump_bytecode: bool,

    /// Trace the runtime of the VM
    #[clap(long, env = "SNIX_TRACE_RUNTIME")]
    pub trace_runtime: bool,

    /// Capture the time (relative to the start time of evaluation) of all events traced with
    /// `--trace-runtime`
    #[clap(long, env = "SNIX_TRACE_RUNTIME_TIMING", requires("trace_runtime"))]
    pub trace_runtime_timing: bool,

    /// Only compile, but do not execute code. This will make Snix act
    /// sort of like a linter.
    #[clap(long)]
    pub compile_only: bool,

    /// Don't print warnings.
    #[clap(long)]
    pub no_warnings: bool,

    /// After the evaluation realises a store path, pull that path's entire runtime closure into the
    /// store (fetching/substituting any members not already present). Off by default. nox's
    /// full-snix mode sets this: its builds read inputs lazily through a FUSE mount, so a
    /// runtime-closure member the build never touched is otherwise left out of the store.
    #[clap(long)]
    pub materialize_closure: bool,

    /// Additional entries to the Nix expression search path, a colon-separated list of directories
    /// used to resolve `<...>`-style lookup paths.
    ///
    /// This option may be given multiple times. Paths added through -I take precedence over
    /// NIX_PATH.
    #[clap(long = "extra-nix-path", short = 'I', action = clap::ArgAction::Append)]
    pub extra_nix_paths: Option<Vec<String>>,

    /// Print "raw" (unquoted) output.
    #[clap(long)]
    pub raw: bool,

    /// Strictly evaluate values, traversing them and forcing e.g.
    /// elements of lists and attribute sets before printing the
    /// return value.
    #[clap(long)]
    pub strict: bool,

    #[clap(flatten)]
    pub service_addrs: ServiceUrlsMemory,

    #[arg(long, env, default_value = "dummy:")]
    pub build_service_addr: String,

    #[clap(flatten)]
    pub tracing_args: snix_tracing::TracingArgs,

    /// An optional path in which Derivations encountered during evaluation
    /// are dumped into, after evaluation. If it doesn't exist, the directory is created.
    ///
    /// Files dumped there are named like they would show up in `/nix/store`,
    /// if produced by Nix. Existing files are not overwritten.
    ///
    /// This is only for debugging and diffing purposes for post-eval inspection;
    /// Snix does not read from these.
    #[clap(long)]
    pub drv_dumpdir: Option<PathBuf>,

    /// A list of web servers used by builtins.fetchurl to obtain files by hash.
    /// Given a hash algorithm ha and a base-16 hash h, Nix will try to download the file
    /// from hashed-mirror/ha/h. This allows files to be downloaded even if they have
    /// disappeared from their original URI.
    #[clap(long, default_values_t = [Url::parse("https://tarballs.nixos.org/").unwrap()])]
    pub hashed_mirrors: Vec<Url>,
}

impl Args {
    pub fn nix_path(&self) -> Option<String> {
        resolve_nix_path(std::env::var("NIX_PATH"), &self.extra_nix_paths)
    }
}

fn resolve_nix_path(
    nix_path: Result<String, std::env::VarError>,
    extra_nix_paths: &Option<Vec<String>>,
) -> Option<String> {
    let nix_path_option = nix_path.ok().filter(|string| !string.is_empty());
    let extra_nix_paths_option = extra_nix_paths.to_owned().map(|vec| vec.join(":"));
    match (nix_path_option, extra_nix_paths_option) {
        (Some(nix_path), Some(mut extra_nix_paths)) => {
            extra_nix_paths.push(':');
            Some(extra_nix_paths + &nix_path)
        }
        (nix_path_option, extra_nix_paths_option) => nix_path_option.or(extra_nix_paths_option),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_nix_path;

    #[test]
    fn test_resolve_nix_path() {
        let nix_path = Ok("/nixpath1:nixpath2=/nixpath2".to_owned());
        let extra_nix_paths = Some(vec!["/extra1".to_owned(), "extra2=/extra2".to_owned()]);
        let expected = Some("/extra1:extra2=/extra2:/nixpath1:nixpath2=/nixpath2".to_owned());
        let actual = resolve_nix_path(nix_path, &extra_nix_paths);
        assert!(actual == expected);
        let nix_path = Err(std::env::VarError::NotPresent);
        let extra_nix_paths = Some(vec!["/extra1".to_owned(), "extra2=/extra2".to_owned()]);
        let expected = Some("/extra1:extra2=/extra2".to_owned());
        let actual = resolve_nix_path(nix_path, &extra_nix_paths);
        assert!(actual == expected);
        let nix_path = Ok("/nixpath1:nixpath2=/nixpath2".to_owned());
        let extra_nix_paths = None;
        let expected = Some("/nixpath1:nixpath2=/nixpath2".to_owned());
        let actual = resolve_nix_path(nix_path, &extra_nix_paths);
        assert!(actual == expected);
        let nix_path = Err(std::env::VarError::NotPresent);
        let extra_nix_paths = None;
        let expected = None;
        let actual = resolve_nix_path(nix_path, &extra_nix_paths);
        assert!(actual == expected);
    }
}
