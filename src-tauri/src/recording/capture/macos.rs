//! macOS capture through Core Audio, ported from the stage 0 spike
//! (`spikes/meeting-recording/src/tap.rs`).
//!
//! - **Microphone:** an IOProc on the default input device. It follows
//!   changes of the default input (headphones plugged in), devices that
//!   disappear, and sample-rate changes, each ending the current segment.
//! - **System audio** (macOS 14.4 and later): a mono global process tap that
//!   excludes ThreeStrands' own process, inside a private aggregate device
//!   that holds only the tap (adding the output device as well recorded
//!   audio twice for meetily), with the default output's UID as its main
//!   sub-device and the tap started automatically. When the default output
//!   changes, the tap and its device are rebuilt for the new one.
//!
//! **Real-time rules.** Each IOProc only copies the callback's samples into
//! a preallocated SPSC ring and pushes one [`Stamp`] into a second ring. It
//! never allocates, locks, logs, or sends. A capture thread per track drains
//! both rings into a [`TrackPipeline`].
//!
//! **Host clock.** `mach_absolute_time` stops while the Mac sleeps, so a
//! callback's `mHostTime` is moved onto `mach_continuous_time` (which keeps
//! counting) before conversion to nanoseconds. [`CaptureBackend::host_time_ns`]
//! uses the same clock, so a recording that spans sleep keeps true
//! timestamps and records the sleep as a gap.
//!
//! **Device notifications.** One process-wide listener on the default input,
//! default output, and device list bumps an epoch; capture threads act once
//! the notifications have been quiet for
//! [`policy::DEVICE_CHANGE_DEBOUNCE`].

use super::pipeline::{Stamp, TrackPipeline};
use super::ring::{ring, Consumer, Producer};
use super::{CaptureBackend, CaptureSession, EventSink, SystemAudioSupport, Track, TrackTarget};
use crate::recording::policy;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioHardwarePropertyTranslatePIDToProcessObject,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject, kAudioStreamPropertyVirtualFormat,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyFormat,
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
    AudioObjectID, AudioObjectPropertyAddress, CATapDescription, CATapMuteBehavior,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatLinearPCM, AudioBuffer, AudioBufferList,
    AudioStreamBasicDescription, AudioTimeStamp, AudioTimeStampFlags,
};
use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSProcessInfo, NSString};
use std::ffi::{c_void, CStr};
use std::mem::size_of;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Once, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Host clock

/// `mach_timebase_info_data_t`: host ticks × numer / denom = nanoseconds.
#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

// libSystem's clock functions, declared here because libc marks its
// bindings deprecated in favor of another crate.
extern "C" {
    /// Host ticks; stops while the Mac sleeps. Core Audio's `mHostTime`
    /// uses this clock.
    fn mach_absolute_time() -> u64;
    /// Like `mach_absolute_time`, but keeps counting while the Mac sleeps.
    fn mach_continuous_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

#[derive(Clone, Copy)]
struct Timebase {
    numer: u64,
    denom: u64,
}

impl Timebase {
    fn get() -> Self {
        static TIMEBASE: OnceLock<Timebase> = OnceLock::new();
        *TIMEBASE.get_or_init(|| {
            let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
            // SAFETY: `info` is a valid out-pointer.
            unsafe { mach_timebase_info(&mut info) };
            if info.numer == 0 || info.denom == 0 {
                Timebase { numer: 1, denom: 1 }
            } else {
                Timebase {
                    numer: info.numer as u64,
                    denom: info.denom as u64,
                }
            }
        })
    }

    fn to_ns(self, ticks: u64) -> u64 {
        (ticks as u128 * self.numer as u128 / self.denom as u128) as u64
    }
}

/// Now on the continuous host clock, in nanoseconds.
pub fn host_now_ns() -> u64 {
    // SAFETY: a plain clock read.
    Timebase::get().to_ns(unsafe { mach_continuous_time() })
}

// ---------------------------------------------------------------------------
// Core Audio property helpers

fn check(status: i32, what: &str) -> Result<(), String> {
    if status == 0 {
        return Ok(());
    }
    let bytes = status.to_be_bytes();
    let code = if bytes.iter().all(|b| b.is_ascii_graphic()) {
        format!("'{}'", String::from_utf8_lossy(&bytes))
    } else {
        status.to_string()
    };
    Err(format!("{what} failed (Core Audio error {code})"))
}

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

/// Reads a fixed-size property, with optional qualifier data.
///
/// # Safety
/// `T` must be the property's exact data type.
unsafe fn get<T: Copy>(
    object: AudioObjectID,
    selector: u32,
    scope: u32,
    qualifier: Option<&[u8]>,
) -> Result<T, String> {
    let mut value: T = std::mem::zeroed();
    let mut size = size_of::<T>() as u32;
    let mut addr = address(selector, scope);
    let (qualifier_size, qualifier_ptr) = qualifier.map_or((0, std::ptr::null()), |q| {
        (q.len() as u32, q.as_ptr() as *const c_void)
    });
    check(
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            qualifier_size,
            qualifier_ptr,
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        ),
        "Reading an audio property",
    )?;
    Ok(value)
}

/// Reads a CFString property (a name or UID).
unsafe fn get_string(object: AudioObjectID, selector: u32) -> Result<String, String> {
    let mut value: *const CFString = std::ptr::null();
    let mut size = size_of::<*const CFString>() as u32;
    let mut addr = address(selector, kAudioObjectPropertyScopeGlobal);
    check(
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        ),
        "Reading an audio device name",
    )?;
    let Some(value) = NonNull::new(value as *mut CFString) else {
        return Ok(String::new());
    };
    // The property follows the Create rule: this call owns the string.
    let owned = CFRetained::from_raw(value);
    Ok(owned.to_string())
}

/// Reads an array-of-AudioObjectID property.
unsafe fn get_ids(object: AudioObjectID, selector: u32, scope: u32) -> Result<Vec<u32>, String> {
    let mut addr = address(selector, scope);
    let mut size = 0u32;
    check(
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        ),
        "Listing audio objects",
    )?;
    let mut ids = vec![0u32; size as usize / size_of::<u32>()];
    if ids.is_empty() {
        return Ok(ids);
    }
    check(
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(ids.as_mut_ptr()).cast(),
        ),
        "Listing audio objects",
    )?;
    ids.truncate(size as usize / size_of::<u32>());
    Ok(ids)
}

fn device_name(device: AudioObjectID) -> String {
    // SAFETY: the name property is a CFString.
    unsafe { get_string(device, kAudioObjectPropertyName) }.unwrap_or_default()
}

fn nominal_rate(device: AudioObjectID) -> Result<f64, String> {
    // SAFETY: the nominal sample rate is a Float64.
    unsafe {
        get::<f64>(
            device,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
            None,
        )
    }
}

fn is_alive(device: AudioObjectID) -> bool {
    // SAFETY: DeviceIsAlive is a UInt32.
    unsafe {
        get::<u32>(
            device,
            kAudioDevicePropertyDeviceIsAlive,
            kAudioObjectPropertyScopeGlobal,
            None,
        )
    }
    .map(|alive| alive != 0)
    .unwrap_or(false)
}

fn default_device(selector: u32) -> Option<AudioObjectID> {
    // SAFETY: the default devices are AudioObjectIDs.
    let device: AudioObjectID =
        unsafe { get(SYSTEM, selector, kAudioObjectPropertyScopeGlobal, None) }.ok()?;
    (device != 0).then_some(device)
}

/// The rate and total channel count of a device's input streams, which must
/// all be 32-bit float linear PCM (the HAL's usual virtual format).
fn input_format(device: AudioObjectID) -> Result<(u32, usize), String> {
    let rate = nominal_rate(device)?;
    // SAFETY: Streams is an array of AudioObjectIDs.
    let streams = unsafe {
        get_ids(
            device,
            kAudioDevicePropertyStreams,
            kAudioObjectPropertyScopeInput,
        )
    }?;
    if streams.is_empty() {
        return Err("The selected microphone has no input.".into());
    }
    let mut channels = 0usize;
    for stream in streams {
        // SAFETY: VirtualFormat is an AudioStreamBasicDescription.
        let format: AudioStreamBasicDescription = unsafe {
            get(
                stream,
                kAudioStreamPropertyVirtualFormat,
                kAudioObjectPropertyScopeGlobal,
                None,
            )
        }?;
        if format.mFormatID != kAudioFormatLinearPCM
            || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
            || format.mBitsPerChannel != 32
        {
            return Err("The microphone uses an audio format ThreeStrands can't record.".into());
        }
        channels += format.mChannelsPerFrame as usize;
    }
    if channels == 0 || !(1_000.0..=768_000.0).contains(&rate) {
        return Err("The microphone reported an unusable audio format.".into());
    }
    Ok((rate.round() as u32, channels))
}

// ---------------------------------------------------------------------------
// Device-change notifications

static DEVICE_EPOCH: AtomicU64 = AtomicU64::new(0);
static LAST_DEVICE_CHANGE_NS: AtomicU64 = AtomicU64::new(0);

unsafe extern "C-unwind" fn on_device_change(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    _client: *mut c_void,
) -> i32 {
    LAST_DEVICE_CHANGE_NS.store(host_now_ns(), Ordering::Relaxed);
    DEVICE_EPOCH.fetch_add(1, Ordering::Release);
    0
}

/// Registers the process-wide device listeners once. They stay for the
/// life of the process, so no callback can outlive its data.
fn ensure_device_listeners() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for selector in [
            kAudioHardwarePropertyDefaultInputDevice,
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioHardwarePropertyDevices,
        ] {
            let mut addr = address(selector, kAudioObjectPropertyScopeGlobal);
            // SAFETY: the listener takes no client data and is never removed.
            let status = unsafe {
                AudioObjectAddPropertyListener(
                    SYSTEM,
                    NonNull::from(&mut addr),
                    Some(on_device_change),
                    std::ptr::null_mut(),
                )
            };
            if let Err(error) = check(status, "Listening for audio device changes") {
                log::warn!(target: "recording", "{error}");
            }
        }
    });
}

/// Tracks which device notifications a capture thread has acted on.
struct DeviceWatch {
    seen: u64,
}

impl DeviceWatch {
    fn new() -> Self {
        ensure_device_listeners();
        Self {
            seen: DEVICE_EPOCH.load(Ordering::Acquire),
        }
    }

    /// True once after a burst of notifications has been quiet for the
    /// debounce interval.
    fn settled_change(&mut self) -> bool {
        let epoch = DEVICE_EPOCH.load(Ordering::Acquire);
        if epoch == self.seen {
            return false;
        }
        let quiet = host_now_ns().saturating_sub(LAST_DEVICE_CHANGE_NS.load(Ordering::Relaxed));
        if quiet < policy::DEVICE_CHANGE_DEBOUNCE.as_nanos() as u64 {
            return false;
        }
        self.seen = epoch;
        true
    }
}

// ---------------------------------------------------------------------------
// IOProc and ring buffers

/// Shared with one IOProc through its client-data pointer. Lives until the
/// IOProc is stopped and destroyed.
struct IoContext {
    samples: Producer<f32>,
    stamps: Producer<Stamp>,
    channels: usize,
    timebase: Timebase,
    /// Callbacks whose buffer layout didn't match `channels`.
    format_errors: AtomicU64,
}

/// The device callback. Real-time: it only reads the buffers, does integer
/// and float arithmetic, and writes into preallocated rings.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let context = &*(client as *const IoContext);
    let list = input.as_ref();
    let count = list.mNumberBuffers as usize;
    if count == 0 {
        return 0;
    }
    let buffers = std::slice::from_raw_parts(list.mBuffers.as_ptr() as *const AudioBuffer, count);
    let mut channels = 0usize;
    let mut frames = usize::MAX;
    for buffer in buffers {
        let width = buffer.mNumberChannels as usize;
        if buffer.mData.is_null() || width == 0 {
            context.format_errors.fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        channels += width;
        frames = frames.min(buffer.mDataByteSize as usize / size_of::<f32>() / width);
    }
    if channels != context.channels {
        context.format_errors.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    if frames == 0 {
        return 0;
    }
    let time = input_time.as_ref();
    let absolute = if time.mFlags.contains(AudioTimeStampFlags::HostTimeValid) {
        time.mHostTime
    } else {
        mach_absolute_time()
    };
    // Ticks slept since boot: moves the stamp onto the continuous clock.
    let slept = mach_continuous_time().wrapping_sub(mach_absolute_time());
    let host_ns = context.timebase.to_ns(absolute.wrapping_add(slept));
    if context.stamps.free() == 0 {
        context.samples.count_drop();
        return 0;
    }
    let pushed = context.samples.push_with(frames * channels, |i| {
        let frame = i / channels;
        let mut channel = i % channels;
        for buffer in buffers {
            let width = buffer.mNumberChannels as usize;
            if channel < width {
                return *(buffer.mData as *const f32).add(frame * width + channel);
            }
            channel -= width;
        }
        0.0
    });
    if pushed {
        context.stamps.push_with(1, |_| Stamp {
            host_ns,
            frames: frames as u32,
        });
    }
    0
}

/// A running IOProc on one device and the consuming ends of its rings.
struct DeviceStream {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    context: *mut IoContext,
    samples: Consumer<f32>,
    stamps: Consumer<Stamp>,
    rate: u32,
    channels: usize,
    running: bool,
    scratch: Vec<f32>,
}

// SAFETY: the raw context pointer is only dereferenced by the IOProc and
// freed by `Drop` after the IOProc is destroyed; the stream itself moves
// between threads only as a whole.
unsafe impl Send for DeviceStream {}

/// Callbacks the stamp ring can hold; far more than fit in the sample ring.
const STAMP_RING: usize = 4096;

impl DeviceStream {
    fn open(device: AudioObjectID, rate: u32, channels: usize) -> Result<Self, String> {
        let ring_samples =
            (rate as f64 * channels as f64 * policy::CAPTURE_RING.as_secs_f64()) as usize;
        let (sample_producer, samples) = ring::<f32>(ring_samples);
        let (stamp_producer, stamps) = ring::<Stamp>(STAMP_RING);
        let context = Box::into_raw(Box::new(IoContext {
            samples: sample_producer,
            stamps: stamp_producer,
            channels,
            timebase: Timebase::get(),
            format_errors: AtomicU64::new(0),
        }));
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: `context` outlives the IOProc (freed in Drop after it is
        // destroyed); `proc_id` is a valid out-pointer.
        let started = unsafe {
            check(
                AudioDeviceCreateIOProcID(
                    device,
                    Some(io_proc),
                    context as *mut c_void,
                    NonNull::from(&mut proc_id),
                ),
                "Opening the audio device",
            )
        };
        let mut stream = Self {
            device,
            proc_id,
            context,
            samples,
            stamps,
            rate,
            channels,
            running: false,
            scratch: Vec::new(),
        };
        if let Err(error) = started {
            stream.proc_id = None;
            return Err(error);
        }
        // SAFETY: the IOProc was created on this device.
        unsafe {
            check(
                AudioDeviceStart(device, proc_id),
                "Starting the audio device",
            )
        }?;
        stream.running = true;
        Ok(stream)
    }

    fn stop(&mut self) {
        if self.running {
            // SAFETY: stopping the IOProc this stream started.
            unsafe { AudioDeviceStop(self.device, self.proc_id) };
            self.running = false;
        }
    }

    /// Moves every complete callback from the rings into the pipeline.
    fn drain(&mut self, pipeline: &mut TrackPipeline) -> std::io::Result<()> {
        while let Some(stamp) = self.stamps.pop() {
            let wanted = stamp.frames as usize * self.channels;
            self.scratch.resize(wanted, 0.0);
            let got = self.samples.pop_into(&mut self.scratch[..wanted]);
            let frames = (got / self.channels) as u32;
            pipeline.push(
                Stamp {
                    host_ns: stamp.host_ns,
                    frames,
                },
                &self.scratch[..frames as usize * self.channels],
            )?;
        }
        Ok(())
    }

    fn dropped(&self) -> u64 {
        self.samples.dropped()
    }

    fn format_errors(&self) -> u64 {
        // SAFETY: the context lives as long as the stream.
        unsafe { (*self.context).format_errors.load(Ordering::Relaxed) }
    }
}

impl Drop for DeviceStream {
    fn drop(&mut self) {
        self.stop();
        // SAFETY: after AudioDeviceStop returns the IOProc is not running;
        // destroying it means it never runs again, so the context can go.
        unsafe {
            if self.proc_id.is_some() {
                AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            }
            drop(Box::from_raw(self.context));
        }
    }
}

// ---------------------------------------------------------------------------
// Process tap

fn any<T: objc2::Message>(object: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(object) }
}

fn ns(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().unwrap_or_default())
}

/// This process's Core Audio process object, or 0 if it has none yet.
fn own_process_object() -> AudioObjectID {
    let pid = std::process::id() as i32;
    // SAFETY: the translation takes a pid_t qualifier and returns an ID.
    unsafe {
        get::<AudioObjectID>(
            SYSTEM,
            kAudioHardwarePropertyTranslatePIDToProcessObject,
            kAudioObjectPropertyScopeGlobal,
            Some(&pid.to_ne_bytes()),
        )
    }
    .unwrap_or(0)
}

/// A process tap, its private aggregate device, and the IOProc on it.
struct Tap {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    stream: Option<DeviceStream>,
    output: AudioObjectID,
    output_name: String,
}

impl Tap {
    /// Creates the tap for the current default output. `own` is this
    /// process's audio object, translated before anything else.
    fn create(own: AudioObjectID) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let output = default_device(kAudioHardwarePropertyDefaultOutputDevice)
            .ok_or("There is no audio output to record from.")?;
        // SAFETY: the device UID is a CFString.
        let output_uid = unsafe { get_string(output, kAudioDevicePropertyDeviceUID) }?;
        let excluded: Vec<Retained<NSNumber>> = if own != 0 {
            vec![NSNumber::new_u32(own)]
        } else {
            Vec::new()
        };
        let excluded = NSArray::from_retained_slice(&excluded);
        // SAFETY: plain Objective-C calls on a freshly allocated description.
        let description = unsafe {
            let description = CATapDescription::initMonoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &excluded,
            );
            description.setName(&NSString::from_str("ThreeStrands Meeting Recording"));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::Unmuted);
            description
        };
        let mut tap: AudioObjectID = 0;
        // SAFETY: `description` is valid and `tap` is a valid out-pointer.
        unsafe {
            check(
                AudioHardwareCreateProcessTap(Some(&description), &mut tap),
                "Creating the system audio tap",
            )
        }?;
        let mut created = Self {
            tap,
            aggregate: 0,
            stream: None,
            output,
            output_name: device_name(output),
        };
        // SAFETY: the tap format is an AudioStreamBasicDescription.
        let format: AudioStreamBasicDescription = unsafe {
            get(
                tap,
                kAudioTapPropertyFormat,
                kAudioObjectPropertyScopeGlobal,
                None,
            )
        }?;
        // SAFETY: reading the UUID the tap description was created with.
        let tap_uid = unsafe { description.UUID().UUIDString() };
        let tap_entry: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_retained_objects(
                &[
                    &*ns(kAudioSubTapUIDKey),
                    &*ns(kAudioSubTapDriftCompensationKey),
                ],
                &[any(tap_uid), any(NSNumber::new_bool(true))],
            );
        let taps = NSArray::from_retained_slice(&[tap_entry]);
        let aggregate_uid = NSString::from_str(&format!(
            "app.threestrands.recording.tap.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let settings: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_retained_objects(
                &[
                    &*ns(kAudioAggregateDeviceNameKey),
                    &*ns(kAudioAggregateDeviceUIDKey),
                    &*ns(kAudioAggregateDeviceMainSubDeviceKey),
                    &*ns(kAudioAggregateDeviceIsPrivateKey),
                    &*ns(kAudioAggregateDeviceIsStackedKey),
                    &*ns(kAudioAggregateDeviceTapAutoStartKey),
                    &*ns(kAudioAggregateDeviceTapListKey),
                ],
                &[
                    any(NSString::from_str("ThreeStrands Meeting Recording")),
                    any(aggregate_uid),
                    any(NSString::from_str(&output_uid)),
                    any(NSNumber::new_bool(true)),
                    any(NSNumber::new_bool(false)),
                    any(NSNumber::new_bool(true)),
                    any(taps),
                ],
            );
        // SAFETY: NSDictionary is toll-free bridged to CFDictionary.
        let cf: &CFDictionary = unsafe { &*(Retained::as_ptr(&settings) as *const CFDictionary) };
        let mut aggregate: AudioObjectID = 0;
        // SAFETY: `cf` is a valid dictionary and `aggregate` an out-pointer.
        unsafe {
            check(
                AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate)),
                "Creating the system audio device",
            )
        }?;
        created.aggregate = aggregate;
        let rate = format.mSampleRate.round() as u32;
        let channels = format.mChannelsPerFrame.max(1) as usize;
        created.stream = Some(DeviceStream::open(aggregate, rate, channels)?);
        Ok(created)
    }

    fn stream(&mut self) -> &mut DeviceStream {
        self.stream.as_mut().expect("a created tap has a stream")
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // The IOProc goes first, then the device that hosts it, then the tap.
        self.stream.take();
        // SAFETY: destroying objects this tap created.
        unsafe {
            if self.aggregate != 0 {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

/// Creates a tap and removes it again, so macOS asks for system audio
/// permission now rather than in the middle of a meeting.
pub fn probe_system_audio_permission() -> Result<(), String> {
    let own = own_process_object();
    Tap::create(own).map(drop)
}

// ---------------------------------------------------------------------------
// Sessions and capture threads

#[derive(Default)]
struct Control {
    stop: AtomicBool,
    paused: AtomicBool,
}

struct MacSession {
    control: Arc<Control>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureSession for MacSession {
    fn pause(&mut self) {
        self.control.paused.store(true, Ordering::SeqCst);
    }

    fn resume(&mut self) {
        self.control.paused.store(false, Ordering::SeqCst);
    }

    fn stop(&mut self) {
        self.control.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MacSession {
    fn drop(&mut self) {
        self.stop();
    }
}

fn save_error(error: std::io::Error) -> String {
    format!("Couldn't save the recording's audio: {error}")
}

/// The loop both tracks share: follow pause, run the track's `step`
/// (drain its device and follow device changes; `poll` is true about once a
/// second), and flush once a second, until stopped or `step` fails.
fn capture_loop(
    control: &Control,
    pipeline: &mut TrackPipeline,
    mut step: impl FnMut(&mut TrackPipeline, bool) -> Result<(), String>,
) {
    let mut last_poll = Instant::now();
    loop {
        let stopping = control.stop.load(Ordering::SeqCst);
        if let Err(error) = pipeline.set_paused(control.paused.load(Ordering::SeqCst)) {
            pipeline.fail(&save_error(error));
            return;
        }
        let now = Instant::now();
        let poll = now.duration_since(last_poll) >= Duration::from_secs(1);
        if poll {
            last_poll = now;
        }
        if let Err(error) = step(pipeline, poll && !stopping) {
            pipeline.fail(&error);
            return;
        }
        if stopping {
            return;
        }
        if let Err(error) = pipeline.maybe_flush(now) {
            pipeline.fail(&save_error(error));
            return;
        }
        std::thread::sleep(policy::CAPTURE_POLL_INTERVAL);
    }
}

fn report_overruns(track: Track, stream: &DeviceStream, reported: &mut u64) {
    let dropped = stream.dropped();
    if dropped > *reported {
        log::warn!(
            target: "recording",
            "{} capture fell behind; {} callback(s) of audio dropped",
            track.as_str(),
            dropped - *reported
        );
        *reported = dropped;
    }
}

fn open_microphone(device: AudioObjectID) -> Result<DeviceStream, String> {
    let (rate, channels) = input_format(device)?;
    DeviceStream::open(device, rate, channels)
        .map_err(|error| format!("Couldn't start the microphone. {error}"))
}

fn run_microphone(
    target: TrackTarget,
    events: EventSink,
    control: Arc<Control>,
    ready: SyncSender<Result<(), String>>,
) {
    let mut watch = DeviceWatch::new();
    let opened = default_device(kAudioHardwarePropertyDefaultInputDevice)
        .ok_or_else(|| "No microphone is connected.".to_string())
        .and_then(open_microphone);
    let mut stream = match opened {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut pipeline =
        match TrackPipeline::new(Track::Mic, &target, stream.rate, stream.channels, events) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                let _ = ready.send(Err(save_error(error)));
                return;
            }
        };
    pipeline.report_device(&device_name(stream.device));
    let _ = ready.send(Ok(()));
    let mut reported = 0;
    let mut format_errors = 0;
    capture_loop(&control, &mut pipeline, |pipeline, poll| {
        stream.drain(pipeline).map_err(save_error)?;
        report_overruns(Track::Mic, &stream, &mut reported);
        let mut follow = watch.settled_change();
        if stream.format_errors() > format_errors {
            format_errors = stream.format_errors();
            follow = true;
        }
        if poll {
            let rate = nominal_rate(stream.device).unwrap_or(0.0).round() as u32;
            follow |= !is_alive(stream.device) || rate != stream.rate;
        }
        if !follow {
            return Ok(());
        }
        let device = default_device(kAudioHardwarePropertyDefaultInputDevice)
            .ok_or("The microphone was disconnected.")?;
        if device == stream.device && is_alive(device) {
            if let Ok((rate, channels)) = input_format(device) {
                if rate == stream.rate && channels == stream.channels {
                    if stream.format_errors() > 0 {
                        // Same device, same format, yet the callbacks don't
                        // match it: fail, and let the service restart the
                        // track with backoff.
                        return Err(
                            "The microphone delivered audio in an unexpected layout.".into()
                        );
                    }
                    return Ok(());
                }
            }
        }
        stream.stop();
        stream.drain(pipeline).map_err(save_error)?;
        stream = open_microphone(device)?;
        format_errors = 0;
        reported = 0;
        pipeline
            .change_device(stream.rate, stream.channels, &device_name(device))
            .map_err(save_error)?;
        Ok(())
    });
    stream.stop();
    let _ = stream.drain(&mut pipeline);
    if let Err(error) = pipeline.finish() {
        log::warn!(target: "recording", "finishing the microphone track: {error}");
    }
}

fn run_system_audio(
    target: TrackTarget,
    events: EventSink,
    control: Arc<Control>,
    ready: SyncSender<Result<(), String>>,
) {
    // The spike found the order matters: translate our PID to its process
    // object before creating anything else.
    let own = own_process_object();
    let mut watch = DeviceWatch::new();
    let mut tap = match Tap::create(own) {
        Ok(tap) => tap,
        Err(error) => {
            let _ = ready.send(Err(format!("Couldn't record system audio. {error}")));
            return;
        }
    };
    let (rate, channels) = (tap.stream().rate, tap.stream().channels);
    let mut pipeline = match TrackPipeline::new(Track::System, &target, rate, channels, events) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            let _ = ready.send(Err(save_error(error)));
            return;
        }
    };
    pipeline.report_device(&tap.output_name);
    let _ = ready.send(Ok(()));
    let mut reported = 0;
    capture_loop(&control, &mut pipeline, |pipeline, _poll| {
        tap.stream().drain(pipeline).map_err(save_error)?;
        report_overruns(Track::System, tap.stream(), &mut reported);
        if tap.stream().format_errors() > 0 {
            // The service restarts the track with backoff.
            return Err("System audio arrived in an unexpected layout.".into());
        }
        if !watch.settled_change() {
            return Ok(());
        }
        let output = default_device(kAudioHardwarePropertyDefaultOutputDevice);
        if output == Some(tap.output) {
            return Ok(());
        }
        tap.stream().stop();
        tap.stream().drain(pipeline).map_err(save_error)?;
        // The replacement is made first, so a failure leaves the old tap to
        // be cleaned up as the track ends; then the old one is removed.
        drop(std::mem::replace(
            &mut tap,
            Tap::create(own).map_err(|e| format!("Couldn't follow the new audio output. {e}"))?,
        ));
        reported = 0;
        let (rate, channels) = (tap.stream().rate, tap.stream().channels);
        let name = tap.output_name.clone();
        pipeline
            .change_device(rate, channels, &name)
            .map_err(save_error)?;
        Ok(())
    });
    tap.stream().stop();
    let _ = tap.stream().drain(&mut pipeline);
    drop(tap);
    if let Err(error) = pipeline.finish() {
        log::warn!(target: "recording", "finishing the system audio track: {error}");
    }
}

// ---------------------------------------------------------------------------
// The backend

pub struct MacBackend {
    system_audio: SystemAudioSupport,
}

/// This Mac's macOS version (major, minor).
pub fn macos_version() -> (u32, u32) {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    (
        version.majorVersion.max(0) as u32,
        version.minorVersion.max(0) as u32,
    )
}

impl MacBackend {
    pub fn new() -> Self {
        let (major, minor) = macos_version();
        let system_audio = if policy::macos_has_system_audio(major, minor) {
            SystemAudioSupport::Available
        } else {
            SystemAudioSupport::NeedsMacos { major, minor }
        };
        Self { system_audio }
    }
}

impl CaptureBackend for MacBackend {
    fn system_audio(&self) -> SystemAudioSupport {
        self.system_audio
    }

    fn host_time_ns(&self) -> u64 {
        host_now_ns()
    }

    fn start(
        &self,
        track: Track,
        target: &TrackTarget,
        events: EventSink,
    ) -> Result<Box<dyn CaptureSession>, String> {
        if track == Track::System && self.system_audio != SystemAudioSupport::Available {
            return Err("System audio needs a newer version of macOS.".into());
        }
        let control = Arc::new(Control::default());
        let (ready_tx, ready_rx) = sync_channel(1);
        let thread_control = control.clone();
        let target = target.clone();
        let thread = std::thread::Builder::new()
            .name(format!("recording-{}", track.as_str()))
            .spawn(move || match track {
                Track::Mic => run_microphone(target, events, thread_control, ready_tx),
                Track::System => run_system_audio(target, events, thread_control, ready_tx),
            })
            .map_err(|error| format!("Couldn't start recording: {error}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(MacSession {
                control,
                thread: Some(thread),
            })),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err("Recording stopped unexpectedly while starting.".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_clock_moves_forward_in_nanoseconds() {
        let first = host_now_ns();
        std::thread::sleep(Duration::from_millis(20));
        let second = host_now_ns();
        let elapsed = second - first;
        assert!(
            (15_000_000..2_000_000_000).contains(&elapsed),
            "{elapsed} ns for a 20 ms sleep"
        );
    }

    #[test]
    fn system_audio_support_follows_the_running_macos() {
        let (major, minor) = macos_version();
        assert!(major >= 11, "macOS {major}.{minor}");
        let expected = if policy::macos_has_system_audio(major, minor) {
            SystemAudioSupport::Available
        } else {
            SystemAudioSupport::NeedsMacos { major, minor }
        };
        assert_eq!(MacBackend::new().system_audio(), expected);
    }

    /// A manual smoke test of real capture: records 5 s of both tracks into
    /// a temp directory and prints what was written. Run it on a Mac with
    /// `cargo test capture_smoke -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs audio devices and permissions; run by hand"]
    fn capture_smoke() {
        use crate::recording::segments::saved_track;
        let dir = crate::recording::test_support::TempDir::new();
        let backend = MacBackend::new();
        let target = TrackTarget {
            audio_dir: dir.path().to_path_buf(),
            origin_host_ns: backend.host_time_ns(),
        };
        let (events, received) = std::sync::mpsc::channel();
        let mic = backend.start(Track::Mic, &target, events.clone());
        let system = backend.start(Track::System, &target, events);
        println!("mic: {:?}", mic.as_ref().map(|_| "started"));
        println!("system: {:?}", system.as_ref().map(|_| "started"));
        std::thread::sleep(Duration::from_secs(5));
        drop(mic);
        drop(system);
        for event in received.try_iter() {
            println!("{event:?}");
        }
        for track in Track::ALL {
            println!("{}: {:?}", track.as_str(), saved_track(dir.path(), track));
        }
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|e| e == "wav") {
                let bytes = std::fs::read(entry.path()).unwrap();
                let peak = bytes[44..]
                    .chunks_exact(2)
                    .map(|p| i16::from_le_bytes([p[0], p[1]]).unsigned_abs())
                    .max()
                    .unwrap_or(0);
                println!("{} peak {peak}", entry.path().display());
            }
            println!(
                "{} {} bytes",
                entry.path().display(),
                entry.metadata().unwrap().len()
            );
        }
    }
}
