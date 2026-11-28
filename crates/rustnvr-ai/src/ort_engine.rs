//! ONNX Runtime as the engine of the built-in recognition, when its library
//! is installed next to RustNVR: 2 to 4 times faster than tract on the same
//! processor (YOLOX-m on an i5-6500: 350 ms instead of 1250 ms;
//! docs/MESURES.md). Nothing to link at build time: `libonnxruntime.so`
//! (the official build of Microsoft, from its PyPI package) is loaded when
//! the first model is, and tract stays the engine without it.
//!
//! Where the library is looked for, in order: `RUSTNVR_ORT_LIB` (a path, or
//! `off` to keep tract), `../lib/rustnvr/libonnxruntime.so` next to the
//! program (the archive's layout: `/usr/local/bin` → `/usr/local/lib`),
//! `/usr/local/lib/rustnvr/` and `/usr/lib/rustnvr/`.
//!
//! One picture is spread over the processor cores that RustNVR may use
//! (those of a LXC, a Docker `cpus` limit or a CPU affinity count, not the
//! machine's), or over `RUSTNVR_AI_THREADS` of them (to leave one free, on
//! a Raspberry Pi for instance). Idle threads do not spin between pictures
//! (ONNX Runtime's default, which burns processor time on a shared machine).

use ort::environment::Environment;
use ort::session::Session;
use ort::value::{Tensor, ValueType};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// File name of the library.
const LIBRARY: &str = "libonnxruntime.so";

/// The environment, made once per process (the library cannot be loaded
/// twice); `Err`: why ONNX Runtime is not used.
static ENVIRONMENT: OnceLock<Result<(Environment, PathBuf), String>> = OnceLock::new();

/// The library to load, or `None` when there is none (or `off`).
fn library() -> Option<PathBuf> {
    if let Some(chosen) = std::env::var_os("RUSTNVR_ORT_LIB") {
        if chosen == "off" || chosen.is_empty() {
            return None;
        }
        return Some(PathBuf::from(chosen));
    }
    let next_to_program = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .map(|bin| bin.join("../lib/rustnvr").join(LIBRARY))
    });
    next_to_program
        .into_iter()
        .chain([
            Path::new("/usr/local/lib/rustnvr").join(LIBRARY),
            Path::new("/usr/lib/rustnvr").join(LIBRARY),
        ])
        .find(|p| p.is_file())
}

/// The environment and the library loaded, `Ok(None)` without a library.
fn environment() -> Result<Option<&'static (Environment, PathBuf)>, String> {
    if ENVIRONMENT.get().is_none() {
        let Some(path) = library() else {
            return Ok(None);
        };
        let made = ort::init_from(&path)
            .map_err(|e| format!("cannot load {}: {e}", path.display()))
            .and_then(|b| {
                b.with_name("rustnvr")
                    .build()
                    .map_err(|e| format!("{}: {e}", path.display()))
            })
            .map(|env| (env, path));
        let _ = ENVIRONMENT.set(made);
    }
    match ENVIRONMENT.get() {
        Some(Ok(env)) => Ok(Some(env)),
        Some(Err(e)) => Err(e.clone()),
        None => Ok(None),
    }
}

/// Most threads accepted from `RUSTNVR_AI_THREADS`.
const MAX_THREADS: usize = 64;

/// Threads for one picture: `RUSTNVR_AI_THREADS` (1 to 64), else the cores
/// RustNVR may use (cgroup quota and affinity included).
pub(crate) fn threads() -> usize {
    std::env::var("RUSTNVR_AI_THREADS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| (1..=MAX_THREADS).contains(n))
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

/// A model opened with ONNX Runtime, the library and the threads it uses.
/// `Ok(None)`: no library installed, tract is used.
pub(crate) fn open(model: &Path) -> Result<Option<(Session, PathBuf, usize)>, String> {
    let Some((env, library)) = environment()? else {
        return Ok(None);
    };
    let threads = threads();
    let fail =
        |e: &dyn std::fmt::Display| format!("ONNX Runtime cannot open {}: {e}", model.display());
    let mut builder = Session::builder(env)
        .map_err(|e| fail(&e))?
        .with_intra_threads(threads)
        .map_err(|e| fail(&e))?
        .with_intra_op_spinning(false)
        .map_err(|e| fail(&e))?;
    let session = builder.commit_from_file(model).map_err(|e| fail(&e))?;
    Ok(Some((session, library.clone(), threads)))
}

/// Side of the square `1×3×side×side` input of `session`, if it fixes one.
pub(crate) fn square_input(session: &Session) -> Option<usize> {
    session
        .inputs()
        .first()
        .and_then(|input| match input.dtype() {
            ValueType::Tensor { shape, .. } => match shape.as_ref() {
                [1, 3, h, w] if h == w && *h > 0 => usize::try_from(*h).ok(),
                _ => None,
            },
            _ => None,
        })
}

/// A YOLOX model opened with ONNX Runtime, the side of its square input
/// (416 when the model does not fix it, as with tract) and the threads it
/// uses. `Ok(None)`: no library installed, tract is used.
pub(crate) fn load(model: &Path) -> Result<Option<(Session, usize, PathBuf, usize)>, String> {
    let Some((session, library, threads)) = open(model)? else {
        return Ok(None);
    };
    let size = square_input(&session).unwrap_or(416);
    Ok(Some((session, size, library, threads)))
}

/// Runs the model on `input` (1×3×size×size): the rows of its first output.
pub(crate) fn run(session: &mut Session, input: Vec<f32>, size: usize) -> Result<Vec<f32>, String> {
    run_input(session, [1, 3, size, size], input, None)
}

/// Runs `session` on one input of `shape`: the values of the output named
/// `output` (the first one when `None`).
pub(crate) fn run_input<T>(
    session: &mut Session,
    shape: [usize; 4],
    input: Vec<T>,
    output: Option<&str>,
) -> Result<Vec<f32>, String>
where
    T: ort::value::PrimitiveTensorElementType + std::fmt::Debug + Clone + 'static,
{
    let index = match output {
        Some(name) => session
            .outputs()
            .iter()
            .position(|o| o.name() == name)
            .ok_or_else(|| format!("the model has no output {name:?}"))?,
        None => 0,
    };
    let tensor =
        Tensor::from_array((shape, input.into_boxed_slice())).map_err(|e| e.to_string())?;
    let outputs = session
        .run(ort::inputs![tensor])
        .map_err(|e| format!("inference: {e}"))?;
    let (_, values) = outputs[index]
        .try_extract_tensor::<f32>()
        .map_err(|e| e.to_string())?;
    Ok(values.to_vec())
}
