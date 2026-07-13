use clap::Parser;
#[cfg(feature = "mimalloc")]
use mimalloc::MiMalloc;
use snix_cli_eval::args::Args;
use snix_cli_eval::repl::Repl;
use snix_cli_eval::{AllowIncomplete, init_io_handle, interpret};
use snix_eval::EvalMode;
use snix_eval::observer::DisassemblingObserver;
use snix_glue::snix_store_io::SnixStoreIO;
use std::io::Write;
use std::rc::Rc;
use std::{fs, path::PathBuf};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Interpret the given code snippet, but only run the Svix compiler
/// on it and return errors and warnings.
fn lint<E: Write + Clone + Send>(
    stderr: &mut E,
    code: &str,
    path: Option<PathBuf>,
    args: &Args,
) -> bool {
    let mut eval_builder = snix_eval::Evaluation::builder_impure();

    if args.strict {
        eval_builder = eval_builder.mode(EvalMode::Strict);
    }

    let source_map = eval_builder.source_map().clone();

    let mut compiler_observer = DisassemblingObserver::new(source_map.clone(), stderr.clone());

    if args.dump_bytecode {
        eval_builder.set_compiler_observer(Some(&mut compiler_observer));
    }

    if args.trace_runtime {
        writeln!(
            stderr,
            "warning: --trace-runtime has no effect with --compile-only"
        )
        .unwrap();
    }

    let eval = eval_builder.build();
    let result = eval.compile_only(code, path);

    if args.display_ast
        && let Some(ref expr) = result.expr
    {
        writeln!(stderr, "AST: {}", snix_eval::pretty_print_expr(expr)).unwrap();
    }

    for error in &result.errors {
        error.fancy_format_write(stderr);
    }

    for warning in &result.warnings {
        warning.fancy_format_write(stderr, &source_map);
    }

    // inform the caller about any errors
    result.errors.is_empty()
}

/// Phase breakdown of the run (see snix_store::perf_stats); consumed by nox-builder.
fn print_phase_stats(started: std::time::Instant) {
    eprintln!(
        "fullsnix-phase-stats {{\"wall\":{{\"secs\":{:.3},\"count\":1}},{}",
        started.elapsed().as_secs_f64(),
        &snix_store::perf_stats::report_json()[1..]
    );
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let t_main = std::time::Instant::now();
    let args = Args::parse();

    // Diagnostic: name the generator being resumed when a panic fires (e.g. the genawaiter
    // "entered unreachable code" from a generator awaiting real I/O).
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let g = snix_eval::current_generator_name();
            eprintln!("PANIC while resuming generator: {:?}", g);
            default_hook(info);
        }));
    }

    let tokio_runtime = tokio::runtime::Runtime::new()?;
    let (mut stdout, mut stderr, io_handle) = tokio_runtime.block_on(async {
        let tracing_handle = snix_tracing::TracingBuilder::default()
            .handle_tracing_args(&args.tracing_args)
            .build()?;

        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
            tracing_handle.get_stdout_writer(),
            tracing_handle.get_stderr_writer(),
            Rc::new(init_io_handle(&args).await),
        ))
    })?;

    if let Some(file) = &args.script {
        run_file(&mut stdout, &mut stderr, io_handle, file.clone(), &args)
    } else if let Some(expr) = &args.expr {
        let success = interpret(
            &mut stderr,
            io_handle,
            expr,
            None,
            &args,
            false,
            AllowIncomplete::RequireComplete,
            None, // TODO(aspen): Pass in --arg/--argstr here
            None,
            None,
        )
        .unwrap()
        .finalize(&mut stdout);
        print_phase_stats(t_main);
        if !success {
            std::process::exit(1);
        }
    } else {
        let mut repl = Repl::new(io_handle, &args);
        repl.run(&mut stdout, &mut stderr)
    }

    Ok(())
}

fn run_file<O: Write, E: Write + Clone + Send>(
    stdout: &mut O,
    stderr: &mut E,
    io_handle: Rc<SnixStoreIO>,
    mut path: PathBuf,
    args: &Args,
) {
    if path.is_dir() {
        path.push("default.nix");
    }
    let contents = fs::read_to_string(&path).expect("failed to read the input file");

    let success = if args.compile_only {
        lint(stderr, &contents, Some(path), args)
    } else {
        interpret(
            stderr,
            io_handle,
            &contents,
            Some(path),
            args,
            false,
            AllowIncomplete::RequireComplete,
            None,
            None,
            None,
        )
        .unwrap()
        .finalize(stdout)
    };

    if !success {
        std::process::exit(1);
    }
}
