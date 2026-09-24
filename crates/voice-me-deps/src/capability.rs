//! Whether the *selected* backend can run on this machine at all
//! (Story 3.3).
//!
//! The runtime and model rows answer "are the files here?". This answers
//! the question in front of it: is there an NVIDIA driver and a GPU new
//! enough for CUDA, a real Vulkan adapter for WebGPU, a key for a remote
//! provider. A selection that cannot run here gets one blocking row that
//! says why in words, together with what the CPU backend would do instead —
//! and the app never quietly runs it on CPU.
//!
//! The hardware probes sit behind [`GpuProbe`] so every matrix row is
//! testable with no GPU at all, the same way the Virtual Microphone's
//! installer is injected. The real probes never crash and never hang the
//! check: a missing driver library is "not found", the driver's own error
//! text is passed through as-is, and each probe runs under a deadline.
//!
//! Neither probe loads an ONNX Runtime library: that only ever happens in
//! the `--probe-runtime` helper process (Decision 1).

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use voice_me_core::{
    BackendSelection, CheckRequest, Dependency, DependencyKind, RemoteProvider,
    SpeechExecutionTarget,
};

/// Decision 5: CUDA's floor is compute capability 6.0 (spec-2-5 Decision 1
/// — ONNX Runtime's prebuilt CUDA kernels start at sm_60 and ship no PTX
/// to JIT an older GPU from). One constant; Story 3.8 may revisit it.
pub const CUDA_MIN_COMPUTE_CAPABILITY: (i32, i32) = (6, 0);

/// How long one hardware probe may take before the check gives up on it.
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// The row's name, as the Dependencies tab and the blocked overlay show it.
pub const CAPABILITY_LABEL: &str = "Selected backend";

/// What selecting the CPU backend would do instead — the half of every
/// capability row that tells the user the way out.
pub const CPU_ALTERNATIVE: &str = "The CPU backend would run speech on this machine's processor \
                                   with Q4 weights instead: slower, but it works here.";

/// One CUDA device, as the driver describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaDevice {
    pub name: String,
    /// Compute capability, `(major, minor)`.
    pub compute_capability: (i32, i32),
}

/// What the CUDA driver said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CudaProbe {
    /// No driver library to load at all (`libcuda.so.1` / `nvcuda.dll`).
    NoDriver,
    /// The driver loaded, and reported this error — its own text, as-is.
    DriverError(String),
    /// The driver answered; these are its devices (possibly none).
    Devices(Vec<CudaDevice>),
}

/// One Vulkan physical device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanAdapter {
    pub name: String,
    /// A `PHYSICAL_DEVICE_TYPE_CPU` device (llvmpipe, SwiftShader): a
    /// software rasteriser, which does not count as a GPU.
    pub cpu: bool,
}

/// What the Vulkan loader said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VulkanProbe {
    /// No Vulkan loader library to load.
    NoLoader,
    /// The loader loaded, and reported this error.
    Error(String),
    /// The loader answered; these are its adapters (possibly none).
    Adapters(Vec<VulkanAdapter>),
}

/// The hardware questions the capability row asks. Injected into
/// `DepsAdapter` so tests can answer them.
pub trait GpuProbe: Send + Sync {
    fn cuda(&self) -> CudaProbe;
    fn vulkan(&self) -> VulkanProbe;
}

/// The real probes: the CUDA driver API through `libloading`, and Vulkan
/// through `ash`'s runtime-loaded entry points.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGpuProbe;

impl GpuProbe for SystemGpuProbe {
    fn cuda(&self) -> CudaProbe {
        within_deadline(probe_cuda).unwrap_or_else(|| {
            CudaProbe::DriverError(format!(
                "the NVIDIA driver did not answer within {} seconds",
                PROBE_DEADLINE.as_secs()
            ))
        })
    }

    fn vulkan(&self) -> VulkanProbe {
        within_deadline(probe_vulkan).unwrap_or_else(|| {
            VulkanProbe::Error(format!(
                "the Vulkan loader did not answer within {} seconds",
                PROBE_DEADLINE.as_secs()
            ))
        })
    }
}

/// Run `probe` on its own thread, giving up after [`PROBE_DEADLINE`].
///
/// A driver that hangs in its initialisation must not hang the check with
/// it. The abandoned thread is left to finish (or not) on its own — there
/// is no safe way to cancel a thread stuck inside a driver, and a leaked
/// thread is far better than a frozen Dependencies tab. A probe that
/// panics is reported the same way as one that never answered.
fn within_deadline<T: Send + 'static>(probe: fn() -> T) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("voice-me-gpu-probe".to_string())
        .spawn(move || {
            let _ = tx.send(probe());
        })
        .ok()?;
    rx.recv_timeout(PROBE_DEADLINE).ok()
}

#[cfg(target_os = "windows")]
const CUDA_DRIVER_LIBRARY: &str = "nvcuda.dll";
#[cfg(not(target_os = "windows"))]
const CUDA_DRIVER_LIBRARY: &str = "libcuda.so.1";

/// `CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR` / `_MINOR`.
const CU_ATTRIBUTE_CC_MAJOR: i32 = 75;
const CU_ATTRIBUTE_CC_MINOR: i32 = 76;

fn probe_cuda() -> CudaProbe {
    use std::ffi::{CStr, c_char, c_int, c_uint};

    type CuInit = unsafe extern "C" fn(c_uint) -> c_int;
    type CuDeviceGetCount = unsafe extern "C" fn(*mut c_int) -> c_int;
    type CuDeviceGet = unsafe extern "C" fn(*mut c_int, c_int) -> c_int;
    type CuDeviceGetName = unsafe extern "C" fn(*mut c_char, c_int, c_int) -> c_int;
    type CuDeviceGetAttribute = unsafe extern "C" fn(*mut c_int, c_int, c_int) -> c_int;
    type CuGetErrorString = unsafe extern "C" fn(c_int, *mut *const c_char) -> c_int;

    // SAFETY: loading the NVIDIA driver library runs its initialisers,
    // which is exactly what every CUDA application does; nothing else in
    // this process has loaded it with different expectations.
    let Ok(library) = (unsafe { libloading::Library::new(CUDA_DRIVER_LIBRARY) }) else {
        return CudaProbe::NoDriver;
    };

    // SAFETY: each symbol is looked up with the signature the CUDA driver
    // API documents for it, and only called while `library` is alive.
    unsafe {
        let symbol = |name: &[u8]| -> Result<*const (), String> {
            library
                .get::<*const ()>(name)
                .map(|symbol| *symbol)
                .map_err(|error| format!("the NVIDIA driver library is incomplete: {error}"))
        };
        let lookup = || -> Result<_, String> {
            Ok((
                std::mem::transmute::<*const (), CuInit>(symbol(b"cuInit\0")?),
                std::mem::transmute::<*const (), CuDeviceGetCount>(symbol(b"cuDeviceGetCount\0")?),
                std::mem::transmute::<*const (), CuDeviceGet>(symbol(b"cuDeviceGet\0")?),
                std::mem::transmute::<*const (), CuDeviceGetName>(symbol(b"cuDeviceGetName\0")?),
                std::mem::transmute::<*const (), CuDeviceGetAttribute>(symbol(
                    b"cuDeviceGetAttribute\0",
                )?),
            ))
        };
        let (cu_init, get_count, get_device, get_name, get_attribute) = match lookup() {
            Ok(functions) => functions,
            Err(error) => return CudaProbe::DriverError(error),
        };
        let error_string = symbol(b"cuGetErrorString\0")
            .ok()
            .map(|pointer| std::mem::transmute::<*const (), CuGetErrorString>(pointer));

        // The driver's own words for `code`, as-is.
        let describe = |code: c_int| -> String {
            if let Some(error_string) = error_string {
                let mut text: *const c_char = std::ptr::null();
                if error_string(code, &mut text) == 0 && !text.is_null() {
                    return CStr::from_ptr(text).to_string_lossy().into_owned();
                }
            }
            format!("CUDA driver error {code}")
        };

        let status = cu_init(0);
        if status != 0 {
            return CudaProbe::DriverError(describe(status));
        }
        let mut count: c_int = 0;
        let status = get_count(&mut count);
        if status != 0 {
            return CudaProbe::DriverError(describe(status));
        }

        let mut devices = Vec::new();
        for ordinal in 0..count {
            let mut device: c_int = 0;
            let status = get_device(&mut device, ordinal);
            if status != 0 {
                return CudaProbe::DriverError(describe(status));
            }
            let mut name = [0 as c_char; 256];
            let name = if get_name(name.as_mut_ptr(), name.len() as c_int, device) == 0 {
                CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned()
            } else {
                format!("CUDA device {ordinal}")
            };
            let (mut major, mut minor): (c_int, c_int) = (0, 0);
            // A failed attribute call would leave (0, 0) and falsely claim
            // "compute capability 0.0"; the driver's own error is the answer.
            for (value, attribute) in [
                (&mut major, CU_ATTRIBUTE_CC_MAJOR),
                (&mut minor, CU_ATTRIBUTE_CC_MINOR),
            ] {
                let status = get_attribute(value, attribute, device);
                if status != 0 {
                    return CudaProbe::DriverError(describe(status));
                }
            }
            devices.push(CudaDevice {
                name,
                compute_capability: (major, minor),
            });
        }
        CudaProbe::Devices(devices)
    }
}

fn probe_vulkan() -> VulkanProbe {
    use ash::vk;

    // SAFETY: loading the Vulkan loader library and resolving its entry
    // points; `ash` owns the library for the lifetime of `entry`.
    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(_) => return VulkanProbe::NoLoader,
    };

    let application = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_0);
    let create_info = vk::InstanceCreateInfo::default().application_info(&application);
    // SAFETY: a minimal instance with no layers or extensions, destroyed
    // below before `entry` is dropped.
    let instance = match unsafe { entry.create_instance(&create_info, None) } {
        Ok(instance) => instance,
        Err(error) => return VulkanProbe::Error(error.to_string()),
    };

    // SAFETY: `instance` is live until `destroy_instance` below, and the
    // physical-device handles are only used while it is.
    let adapters = unsafe {
        let result = instance.enumerate_physical_devices().map(|devices| {
            devices
                .into_iter()
                .map(|device| {
                    let properties = instance.get_physical_device_properties(device);
                    VulkanAdapter {
                        name: properties
                            .device_name_as_c_str()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| "an unnamed adapter".to_string()),
                        cpu: properties.device_type == vk::PhysicalDeviceType::CPU,
                    }
                })
                .collect::<Vec<_>>()
        });
        instance.destroy_instance(None);
        result
    };

    match adapters {
        Ok(adapters) => VulkanProbe::Adapters(adapters),
        Err(error) => VulkanProbe::Error(error.to_string()),
    }
}

/// The capability row for `request`: `None` for a CPU selection (a normal
/// state that never warns, UX-DR18), a ready row naming what was found when
/// a GPU selection can run, and a blocking row saying why otherwise.
///
/// Only the probe the selection needs is run: a CPU selection asks the
/// hardware nothing at all.
pub fn capability_row(request: &CheckRequest, probe: &dyn GpuProbe) -> Option<Dependency> {
    let selection = &request.selection;
    match selection {
        BackendSelection::Local { target, .. } => match target {
            SpeechExecutionTarget::Cpu => None,
            SpeechExecutionTarget::Cuda => Some(cuda_row(
                selection,
                request.backend.device.unwrap_or(0),
                probe.cuda(),
            )),
            SpeechExecutionTarget::WebGpu => Some(webgpu_row(selection, probe.vulkan())),
        },
        BackendSelection::Remote(provider) => {
            if !request.has_api_key {
                return Some(cannot_run(
                    selection,
                    &format!(
                        "{} has no API key — add one in Settings → Backend.",
                        provider.label()
                    ),
                ));
            }
            match provider {
                // Story 3.6: a key is all DeepInfra needs here. Nothing is
                // probed — no byte goes to the provider before the user
                // has confirmed what is sent.
                RemoteProvider::DeepInfra => None,
                // Story 3.7 brings fal.ai generation.
                RemoteProvider::FalAi => Some(cannot_run(
                    selection,
                    &format!(
                        "Remote generation through {} arrives in a later voice-me release.",
                        provider.label()
                    ),
                )),
                // Story 3.14: key → region → voice, and nothing probed —
                // the voice list is fetched by the composition root, not
                // by the check.
                RemoteProvider::Azure if !request.has_region => Some(cannot_run(
                    selection,
                    "Azure has no region — add one in Settings → Backend.",
                )),
                RemoteProvider::Azure if !request.has_voice => Some(cannot_run(
                    selection,
                    "Azure has no voice selected — pick one in Settings → Backend.",
                )),
                RemoteProvider::Azure => None,
            }
        }
        // Story 3.12: on Linux the System voice's readiness is the eSpeak
        // NG row ([`system_voice_engine_row`]), not a capability row.
        // Elsewhere it has no engine yet (Windows is Story 3.13).
        BackendSelection::SystemVoice => {
            if cfg!(target_os = "linux") {
                None
            } else {
                Some(cannot_run(
                    selection,
                    "The System voice on this system arrives in a later voice-me release.",
                ))
            }
        }
        // Story 3.15: on Linux Piper's readiness is its runtime, voice and
        // eSpeak NG rows. Elsewhere it has no engine yet (Windows is Story
        // 3.16).
        BackendSelection::Piper => {
            if cfg!(target_os = "linux") {
                None
            } else {
                Some(cannot_run(
                    selection,
                    "Piper on this system arrives in a later voice-me release.",
                ))
            }
        }
    }
}

/// The eSpeak NG row's name.
pub const SYSTEM_VOICE_ENGINE_LABEL: &str = "eSpeak NG";

/// The System voice's engine row (Story 3.12): ready, naming where
/// `espeak-ng` was found, or missing and speech-blocking with manual steps
/// only — it is a system package, which voice-me never installs.
/// `install_step` is the distribution's command, in words; `used_by` ends
/// the missing sentence with who needs it (Story 3.15: Piper, too).
pub fn system_voice_engine_row(
    found: Option<&Path>,
    install_step: &str,
    used_by: &str,
) -> Dependency {
    match found {
        Some(path) => Dependency::ready(
            DependencyKind::SystemVoiceEngine,
            SYSTEM_VOICE_ENGINE_LABEL,
            format!("Found at {}.", path.display()),
        ),
        None => Dependency::missing(
            DependencyKind::SystemVoiceEngine,
            SYSTEM_VOICE_ENGINE_LABEL,
            format!("The espeak-ng program is not on PATH. {used_by}"),
        )
        .manual([install_step.to_string(), "Press Check again.".to_string()]),
    }
}

/// The CUDA row, judged on the device the engine will actually build on —
/// `device_id`, which is what `ExecutionTarget::Cuda` registers — never on
/// whichever device happens to pass.
fn cuda_row(selection: &BackendSelection, device_id: u32, probe: CudaProbe) -> Dependency {
    let devices = match probe {
        CudaProbe::NoDriver => return cannot_run(selection, "No NVIDIA driver found."),
        CudaProbe::DriverError(error) => {
            return cannot_run(
                selection,
                &format!("The NVIDIA driver reported an error: {error}"),
            );
        }
        CudaProbe::Devices(devices) => devices,
    };
    if devices.is_empty() {
        return cannot_run(selection, "The NVIDIA driver found no CUDA device.");
    }
    let Some(device) = devices.get(device_id as usize) else {
        return cannot_run(
            selection,
            &format!(
                "CUDA device {device_id} does not exist; the NVIDIA driver sees {} device(s).",
                devices.len()
            ),
        );
    };

    let (floor_major, floor_minor) = CUDA_MIN_COMPUTE_CAPABILITY;
    let (major, minor) = device.compute_capability;
    if device.compute_capability >= CUDA_MIN_COMPUTE_CAPABILITY {
        return Dependency::ready(
            DependencyKind::BackendCapability,
            CAPABILITY_LABEL,
            format!(
                "{} can run here: {} (compute capability {major}.{minor}).",
                selection.label(),
                device.name
            ),
        );
    }
    cannot_run(
        selection,
        &format!(
            "{} has compute capability {major}.{minor} < {floor_major}.{floor_minor} required.",
            device.name
        ),
    )
}

fn webgpu_row(selection: &BackendSelection, probe: VulkanProbe) -> Dependency {
    let adapters = match probe {
        VulkanProbe::NoLoader => return cannot_run(selection, "No Vulkan loader found."),
        VulkanProbe::Error(error) => {
            return cannot_run(selection, &format!("Vulkan reported an error: {error}"));
        }
        VulkanProbe::Adapters(adapters) => adapters,
    };

    if let Some(gpu) = adapters.iter().find(|adapter| !adapter.cpu) {
        return Dependency::ready(
            DependencyKind::BackendCapability,
            CAPABILITY_LABEL,
            format!("{} can run here: {}.", selection.label(), gpu.name),
        );
    }
    if adapters.is_empty() {
        return cannot_run(selection, "Vulkan found no adapter at all.");
    }
    let software = adapters
        .iter()
        .map(|adapter| adapter.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    cannot_run(
        selection,
        &format!("Vulkan found only a CPU-type adapter ({software}), which is not a GPU."),
    )
}

/// The blocking row: the selection, the reason in words, and the way out.
fn cannot_run(selection: &BackendSelection, reason: &str) -> Dependency {
    let mut row = Dependency::missing(
        DependencyKind::BackendCapability,
        CAPABILITY_LABEL,
        format!(
            "{} can't run here: {reason} {CPU_ALTERNATIVE}",
            selection.label()
        ),
    );
    // Nothing to install and no steps to follow: the row's one action is
    // "Use CPU backend", which the Dependencies tab renders for this kind.
    row.automatable = false;
    row
}

#[cfg(test)]
mod tests {
    //! Every capability row of the I/O & Edge-Case Matrix, against a fake
    //! probe — no GPU, driver or Vulkan loader needed.

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use voice_me_core::{DependencyStatus, SpeechBackend};

    use super::*;

    struct FakeProbe {
        cuda: CudaProbe,
        vulkan: VulkanProbe,
        asked: AtomicUsize,
    }

    impl FakeProbe {
        fn new(cuda: CudaProbe, vulkan: VulkanProbe) -> Self {
            Self {
                cuda,
                vulkan,
                asked: AtomicUsize::new(0),
            }
        }
    }

    impl GpuProbe for FakeProbe {
        fn cuda(&self) -> CudaProbe {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.cuda.clone()
        }

        fn vulkan(&self) -> VulkanProbe {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.vulkan.clone()
        }
    }

    fn good_gpu() -> FakeProbe {
        FakeProbe::new(
            CudaProbe::Devices(vec![CudaDevice {
                name: "NVIDIA GeForce RTX 3060".to_string(),
                compute_capability: (8, 6),
            }]),
            VulkanProbe::Adapters(vec![VulkanAdapter {
                name: "NVIDIA GeForce RTX 3060".to_string(),
                cpu: false,
            }]),
        )
    }

    fn local(target: SpeechExecutionTarget) -> CheckRequest {
        CheckRequest {
            backend: SpeechBackend::for_target(target),
            selection: BackendSelection::Local {
                runtime: Some(PathBuf::from("/opt/ort/libonnxruntime.so")),
                target,
            },
            has_api_key: false,
            has_region: false,
            has_voice: false,
            piper_voice: None,
        }
    }

    fn remote(has_api_key: bool) -> CheckRequest {
        CheckRequest {
            backend: SpeechBackend::CPU,
            selection: BackendSelection::Remote(RemoteProvider::DeepInfra),
            has_api_key,
            has_region: false,
            has_voice: false,
            piper_voice: None,
        }
    }

    fn assert_blocks(row: &Dependency, reason: &str) {
        assert_eq!(row.kind, DependencyKind::BackendCapability);
        assert_eq!(row.status, DependencyStatus::Missing);
        assert!(row.kind.blocks_speech());
        assert!(!row.automatable, "the way out is Use CPU, not Install");
        assert!(row.detail.contains(reason), "{}", row.detail);
        assert!(
            row.detail.contains(CPU_ALTERNATIVE),
            "every blocking row says what the CPU backend would do: {}",
            row.detail
        );
    }

    /// UX-DR18: CPU on a machine with a good GPU is a normal state — no
    /// row, no warning, and the hardware is not even asked.
    #[test]
    fn cpu_with_a_good_gpu_present_has_no_capability_row() {
        let probe = good_gpu();

        assert_eq!(capability_row(&CheckRequest::cpu(), &probe), None);
        assert_eq!(
            capability_row(&local(SpeechExecutionTarget::Cpu), &probe),
            None
        );
        assert_eq!(probe.asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cuda_with_no_driver_says_so_and_offers_cpu() {
        let probe = FakeProbe::new(CudaProbe::NoDriver, VulkanProbe::NoLoader);

        let row = capability_row(&local(SpeechExecutionTarget::Cuda), &probe).unwrap();

        assert_blocks(&row, "can't run here: No NVIDIA driver found.");
        assert!(
            row.detail.starts_with("CUDA (libonnxruntime.so)"),
            "{}",
            row.detail
        );
    }

    #[test]
    fn cuda_on_an_old_gpu_names_the_device_and_the_floor() {
        let probe = FakeProbe::new(
            CudaProbe::Devices(vec![CudaDevice {
                name: "NVIDIA GeForce GTX 750 Ti".to_string(),
                compute_capability: (5, 0),
            }]),
            VulkanProbe::NoLoader,
        );

        let row = capability_row(&local(SpeechExecutionTarget::Cuda), &probe).unwrap();

        assert_blocks(&row, "NVIDIA GeForce GTX 750 Ti");
        assert!(row.detail.contains("5.0 < 6.0 required"), "{}", row.detail);
    }

    /// The engine builds on device 0 (no device chosen): a capable device 1
    /// does not make an sm_50 device 0 runnable.
    #[test]
    fn cuda_is_judged_on_the_device_the_engine_will_use() {
        let probe = FakeProbe::new(
            CudaProbe::Devices(vec![
                CudaDevice {
                    name: "Quadro M1200".to_string(),
                    compute_capability: (5, 0),
                },
                CudaDevice {
                    name: "NVIDIA GeForce RTX 3060".to_string(),
                    compute_capability: (8, 6),
                },
            ]),
            VulkanProbe::NoLoader,
        );

        let row = capability_row(&local(SpeechExecutionTarget::Cuda), &probe).unwrap();
        assert_blocks(
            &row,
            "Quadro M1200 has compute capability 5.0 < 6.0 required",
        );
        assert!(!row.detail.contains("RTX 3060"), "{}", row.detail);

        let mut on_device_one = local(SpeechExecutionTarget::Cuda);
        on_device_one.backend.device = Some(1);
        let row = capability_row(&on_device_one, &probe).unwrap();
        assert_eq!(row.status, DependencyStatus::Ready);
        assert!(row.detail.contains("RTX 3060"), "{}", row.detail);
    }

    #[test]
    fn a_driver_error_is_passed_through_as_is() {
        let probe = FakeProbe::new(
            CudaProbe::DriverError("CUDA driver version is insufficient".to_string()),
            VulkanProbe::NoLoader,
        );

        let row = capability_row(&local(SpeechExecutionTarget::Cuda), &probe).unwrap();

        assert_blocks(&row, "CUDA driver version is insufficient");
    }

    #[test]
    fn cuda_on_a_capable_gpu_is_ready_and_names_it() {
        let row = capability_row(&local(SpeechExecutionTarget::Cuda), &good_gpu()).unwrap();

        assert_eq!(row.status, DependencyStatus::Ready);
        assert!(row.detail.contains("RTX 3060"), "{}", row.detail);
        assert!(row.detail.contains("8.6"), "{}", row.detail);
    }

    #[test]
    fn webgpu_with_no_vulkan_loader_says_so() {
        let probe = FakeProbe::new(CudaProbe::NoDriver, VulkanProbe::NoLoader);

        let row = capability_row(&local(SpeechExecutionTarget::WebGpu), &probe).unwrap();

        assert_blocks(&row, "No Vulkan loader found.");
    }

    /// llvmpipe is a Vulkan device, but not a GPU: it must not count.
    #[test]
    fn webgpu_with_only_a_software_adapter_names_what_was_found() {
        let probe = FakeProbe::new(
            CudaProbe::NoDriver,
            VulkanProbe::Adapters(vec![VulkanAdapter {
                name: "llvmpipe (LLVM 19.1.7, 256 bits)".to_string(),
                cpu: true,
            }]),
        );

        let row = capability_row(&local(SpeechExecutionTarget::WebGpu), &probe).unwrap();

        assert_blocks(&row, "llvmpipe (LLVM 19.1.7, 256 bits)");
    }

    #[test]
    fn webgpu_on_a_real_adapter_is_ready() {
        let row = capability_row(&local(SpeechExecutionTarget::WebGpu), &good_gpu()).unwrap();

        assert_eq!(row.status, DependencyStatus::Ready);
    }

    /// Story 3.12: a missing `espeak-ng` blocks speech, with the install
    /// command as a manual step and no Install.
    #[test]
    fn a_missing_espeak_ng_blocks_with_manual_steps_only() {
        let row = system_voice_engine_row(
            None,
            "Install it from a terminal: sudo apt install espeak-ng",
            "Piper reads text through it.",
        );
        assert!(row.detail.contains("Piper"), "{}", row.detail);

        assert_eq!(row.kind, DependencyKind::SystemVoiceEngine);
        assert_eq!(row.status, DependencyStatus::Missing);
        assert!(row.kind.blocks_speech());
        assert!(!row.automatable, "a system package: steps, never Install");
        assert!(row.manual_steps[0].contains("sudo apt install espeak-ng"));
        assert_eq!(row.label, "eSpeak NG");
    }

    #[test]
    fn a_found_espeak_ng_is_ready_and_names_its_path() {
        let row =
            system_voice_engine_row(Some(Path::new("/usr/bin/espeak-ng")), "unused", "unused");

        assert_eq!(row.status, DependencyStatus::Ready);
        assert!(row.detail.contains("/usr/bin/espeak-ng"), "{}", row.detail);
    }

    fn system_voice() -> CheckRequest {
        CheckRequest {
            backend: SpeechBackend::CPU,
            selection: BackendSelection::SystemVoice,
            has_api_key: false,
            has_region: false,
            has_voice: false,
            piper_voice: None,
        }
    }

    /// On Linux the System voice's readiness is its engine row, so there
    /// is no capability row, and no hardware is asked.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_system_voice_on_linux_has_no_capability_row() {
        let probe = good_gpu();
        assert_eq!(capability_row(&system_voice(), &probe), None);
        assert_eq!(probe.asked.load(Ordering::SeqCst), 0);
    }

    /// Elsewhere it cannot run yet, and says so.
    #[test]
    #[cfg(not(target_os = "linux"))]
    fn the_system_voice_off_linux_cannot_run_yet() {
        let row = capability_row(&system_voice(), &good_gpu()).unwrap();
        assert_blocks(&row, "arrives in a later voice-me release");
    }

    #[test]
    fn a_remote_provider_with_no_key_points_at_the_backend_tab() {
        let row = capability_row(&remote(false), &good_gpu()).unwrap();

        assert_blocks(
            &row,
            "DeepInfra has no API key — add one in Settings → Backend",
        );
    }

    #[test]
    fn deepinfra_with_a_key_can_run_and_nothing_is_probed() {
        assert_eq!(capability_row(&remote(true), &good_gpu()), None);
    }

    #[test]
    fn fal_ai_with_a_key_is_still_honest_that_it_cannot_generate_yet() {
        let request = CheckRequest {
            selection: BackendSelection::Remote(RemoteProvider::FalAi),
            ..remote(true)
        };
        let row = capability_row(&request, &good_gpu()).unwrap();

        assert_blocks(
            &row,
            "Remote generation through fal.ai arrives in a later voice-me release",
        );
    }

    fn azure(has_api_key: bool, has_region: bool, has_voice: bool) -> CheckRequest {
        CheckRequest {
            backend: SpeechBackend::CPU,
            selection: BackendSelection::Remote(RemoteProvider::Azure),
            has_api_key,
            has_region,
            has_voice,
            piper_voice: None,
        }
    }

    /// Story 3.14: key, then region, then voice — each its own blocking
    /// row, and nothing probed.
    #[test]
    fn azure_blocks_on_key_then_region_then_voice() {
        let probe = good_gpu();
        let row = capability_row(&azure(false, false, false), &probe).unwrap();
        assert_blocks(
            &row,
            "Azure has no API key — add one in Settings → Backend.",
        );
        let row = capability_row(&azure(true, false, false), &probe).unwrap();
        assert_blocks(&row, "Azure has no region — add one in Settings → Backend.");
        let row = capability_row(&azure(true, true, false), &probe).unwrap();
        assert_blocks(
            &row,
            "Azure has no voice selected — pick one in Settings → Backend.",
        );
        assert_eq!(capability_row(&azure(true, true, true), &probe), None);
        assert_eq!(probe.asked.load(Ordering::SeqCst), 0);
    }

    /// The deadline wrapper hands back what the probe returned.
    #[test]
    fn a_prompt_probe_answers_within_the_deadline() {
        assert_eq!(within_deadline(|| 7_u8), Some(7));
    }

    /// The real probes against whatever this machine has. Ignored by
    /// default because the answer is machine-specific; run with
    /// `--ignored --nocapture` to see what the capability row will say.
    #[test]
    #[ignore]
    fn the_system_probes_answer_without_panicking_or_hanging() {
        let probe = SystemGpuProbe;
        println!("cuda: {:?}", probe.cuda());
        println!("vulkan: {:?}", probe.vulkan());
    }
}
