use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Output, Stdio},
};

use tokio::process::Command;

use crate::sandbox::{InputsProvider, SandboxSpec};

const COMMON_BWRAP_ARGS: &[&str] = &[
    "--unshare-uts",
    "--unshare-ipc",
    "--unshare-pid",
    "--die-with-parent",
    "--as-pid-1",
    "--clearenv",
    "--tmpfs",
    "/",
    "--dev",
    "/dev",
    "--proc",
    "/proc",
    "--tmpfs",
    "/tmp",
];

/// Only with [SandboxSpec::userns]: unshare a user namespace and become nixbld inside it.
/// (--uid/--gid require --unshare-user; without it the build runs as the invoking user.)
const USERNS_BWRAP_ARGS: &[&str] = &["--unshare-user", "--uid", "1000", "--gid", "100"];

const ETC_PASSWD: &[u8] = b"
root:x:0:0:Nix build user:/build:/noshell
nixbld:x:1000:100:Nix build user:/build:/noshell
nobody:x:65534:65534:Nobody:/:/noshell
";

const ETC_GROUP: &[u8] = b"
root:x:0:
nixbld:!:100:
nogroup:x:65534:
";

const ETC_HOSTS: &[u8] = b"
127.0.0.1 localhost
::1 localhost
";

const ETC_NSSWITCH: &[u8] = b"
hosts: files dns
services: files
";

/// Bubblewrap based sandbox executor.
///
/// It executes the sandbox command in separate uts, ipc, pid and user namespaces,
/// always runs as uid=1000(nixbld) and gid=100(nixbld) inside the namespace. Provides sane
/// defaults for various `/etc` files.
///
/// Network is optionally disabled with a separate network namespace based on the value of
/// [SandboxSpec::allow_network].
///
/// The root filesystem is tmpfs, has /dev and /proc.
///
/// The rest of the filesystem is based on the [SandboxSpec::scratches], [SandboxSpec::additional_files]
/// and [SandboxSpec::inputs_provider].
///
/// # Scratches
///
/// A list of read-write directories available inside the sandbox, these directories are also left
/// available on the host after the sandbox has finished.
///
/// # Additional files
///
/// A list of read-write files whose path currently *must* resolve into one of the Scratches.
///
/// # Build Inputs([SandboxSpec::inputs_provider])
///
/// A read-only directory that contains any files required by the sandboxed command, e.g
/// `/nix/store`.
/// Before the sandbox starts, the [SandboxSpec::inputs_provider] will have a chance to populate
/// this directory and clean up after the sandbox is stopped.
///
/// **Note**: If the build inputs directory overlaps with any of the scratches, an overlayfs mount
/// will be created for that scratch so it remains writable, i.e. the sandboxed command can create
/// new files/directories.
pub struct Bwrap {
    host_workdir: PathBuf,
    args: Vec<OsString>,
    inputs_provider: InputsProvider,
}

/// The result of running the sandbox.
pub struct SandboxOutcome {
    output: Output,
    scratch_dir: PathBuf,
}

impl SandboxOutcome {
    /// Status code, stderr, stdout, etc.
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// Allows finding outputs produced by the sandboxed command.
    ///
    /// The command must write into one of the scratches.
    pub fn find_path(&self, path: impl AsRef<Path>) -> Option<PathBuf> {
        let path = self.scratch_dir.join(path);
        // Exists follows symlinks so may return false incorrectly, as nix builds are apparently
        // allowed to produce broken symlinks as their $out...
        // i.e. `runCommand "test" {} "ln -s IdontExist $out"` is a valid nix build.
        //
        // Additionally, SandboxOutcome values are handed out by builds **after** unmounting the
        // fuse store, which means that even valid symlinks can be "broken" during ingestion.
        if path.is_symlink() || path.exists() {
            Some(path)
        } else {
            None
        }
    }
}

impl Bwrap {
    // TODO(#132): support streaming std{err,out}
    /// Run the sandbox and return the result.
    pub async fn run(mut self) -> std::io::Result<SandboxOutcome> {
        let _guard = self
            .inputs_provider
            .provide_inputs(self.host_workdir.join("host_inputs_dir"))?;

        Ok(SandboxOutcome {
            output: Command::new("bwrap")
                .args(self.args)
                // Make sure we've closed stdin otherwise builds can hang forever blocked on std io.
                .stdin(Stdio::null())
                .output()
                .await?,
            scratch_dir: self.host_workdir.join("scratches"),
        })
    }

    /// Constructor.
    pub fn initialize(spec: SandboxSpec) -> std::io::Result<Bwrap> {
        let scratch_dir = spec.host_workdir().join("scratches");
        fs::create_dir_all(&scratch_dir)?;
        let mut args: Vec<OsString> = COMMON_BWRAP_ARGS.iter().map(|s| s.into()).collect();
        if spec.userns() {
            args.extend(USERNS_BWRAP_ARGS.iter().map(OsString::from));
        }
        if !spec.allow_network() {
            args.push("--unshare-net".into());
        }
        for env in spec.env_vars() {
            args.extend([
                "--setenv".into(),
                env.key.clone().into(),
                str::from_utf8(&env.value)
                    .expect("invalid string in env")
                    .into(),
            ]);
        }

        let host_inputs_dir = spec.host_workdir().join("host_inputs_dir");
        fs::create_dir_all(&host_inputs_dir)?;
        let inputs_dir = spec.inputs_provider().inputs_dir().to_path_buf();
        // How the build's inputs are provided. Three modes:
        //  - default (external_inputs None): a per-build castore FUSE the provider mounts at
        //    host_inputs_dir, bound at inputs_dir and made writable by the overlay below.
        //  - external whole-store (external_inputs set, no names): bind the whole already-mounted
        //    store (e.g. nox-mount) as the overlay lower — reuses that cache, no per-build FUSE, but
        //    exposes the output path too (only safe when the output isn't already in the store).
        //  - external declared-inputs (external_inputs + external_input_names): inputs_dir is a
        //    writable scratch, and only the build's declared input paths are bound read-only into it
        //    from the mount. The output path is never in view, so the build creates it fresh — this
        //    is the correct, pure form. bwrap binds inside its own userns, so no extra privilege.
        let declared =
            spec.external_inputs().is_some() && !spec.external_input_names().is_empty();
        let inputs_src: OsString = match spec.external_inputs() {
            Some(ext) => ext.into(),
            None => Path::new("/").join(&host_inputs_dir).into(),
        };
        if !declared {
            // Bind the whole inputs source at inputs_dir; writability (when inputs_dir is a scratch)
            // is provided by the overlay below.
            args.extend([
                "--ro-bind".into(),
                inputs_src.clone(),
                Path::new("/").join(&inputs_dir).into(),
            ]);
        }
        for scratch in spec.scratches() {
            let scratch_path = scratch_dir.join(scratch);
            fs::create_dir_all(&scratch_path)?;
            if *scratch == inputs_dir {
                if declared {
                    // inputs_dir is a plain writable scratch; declared inputs are bound in below.
                    args.extend([
                        "--bind".into(),
                        scratch_path.into(),
                        Path::new("/").join(scratch).into(),
                    ]);
                } else {
                    let overlay_workdir = spec.host_workdir().join("overlay_workdir");
                    fs::create_dir_all(&overlay_workdir)?;
                    args.extend([
                        "--overlay-src".into(),
                        inputs_src.clone(),
                        "--overlay".into(),
                        scratch_path.into(),
                        overlay_workdir.into(),
                        Path::new("/").join(&inputs_dir).into(),
                    ]);
                }
            } else {
                args.extend([
                    "--bind".into(),
                    scratch_path.into(),
                    Path::new("/").join(scratch).into(),
                ]);
            }
        }
        if declared {
            // Bind each declared input read-only from the external mount into inputs_dir.
            let ext = spec
                .external_inputs()
                .expect("declared implies external_inputs");
            for name in spec.external_input_names() {
                args.extend([
                    "--ro-bind".into(),
                    ext.join(name).into(),
                    Path::new("/").join(&inputs_dir).join(name).into(),
                ]);
            }
        }
        args.extend([
            "--chdir".into(),
            Path::new("/").join(spec.sandbox_workdir()).into(),
        ]);

        if let Some(shell) = spec.provide_shell() {
            args.extend_from_slice(&["--ro-bind".into(), shell.into(), "/bin/sh".into()]);
        }

        for file in spec.additional_files() {
            let mut found = false;
            for scratch in spec.scratches() {
                if file.path.starts_with(scratch) {
                    found = true;
                }
            }
            if !found {
                return Err(std::io::Error::other(format!(
                    "Additional file does not belong to any scratch: {:?}",
                    file.path
                )));
            }
            // TODO: prevent files from escaping the sandbox, i.e. don't allow additional files
            // of this form: build/../../hello.
            let file_path = scratch_dir.join(&file.path);
            fs::create_dir_all(file_path.parent().expect("parent"))?;
            fs::write(&file_path, &file.contents)?;
        }
        let etc = &spec.host_workdir().join("etc");
        fs::create_dir_all(etc)?;
        fs::write(etc.join("passwd"), ETC_PASSWD)?;
        fs::write(etc.join("group"), ETC_GROUP)?;
        fs::write(etc.join("hosts"), ETC_HOSTS)?;
        fs::write(etc.join("nsswitch.conf"), ETC_NSSWITCH)?;

        args.extend([
            "--ro-bind".into(),
            etc.join("passwd").into(),
            "/etc/passwd".into(),
            "--ro-bind".into(),
            etc.join("group").into(),
            "/etc/group".into(),
        ]);
        if spec.allow_network() {
            // -try: minimal containers (e.g. dockerTools images) may lack /etc/services or even
            // /etc/hosts; a plain --ro-bind of a missing source makes bwrap itself fail.
            args.extend([
                "--ro-bind-try".into(),
                "/etc/hosts".into(),
                "/etc/hosts".into(),
                "--ro-bind-try".into(),
                "/etc/resolv.conf".into(),
                "/etc/resolv.conf".into(),
                "--ro-bind-try".into(),
                "/etc/services".into(),
                "/etc/services".into(),
                "--ro-bind".into(),
                etc.join("nsswitch.conf").into(),
                "/etc/nsswitch.conf".into(),
            ]);
        } else {
            // Use predefined /etc/hosts like nix does.
            // Among other things it is required for libuv getaddrinfo() tests to pass.
            args.extend([
                "--ro-bind".into(),
                etc.join("hosts").into(),
                "/etc/hosts".into(),
            ]);
        }
        args.extend(spec.command().into_iter().map(|s| s.into()));

        Ok(Self {
            host_workdir: spec.host_workdir().into(),
            args,
            inputs_provider: spec.into(),
        })
    }
}
