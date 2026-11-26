//! macOS system audio through a Core Audio process tap (macOS 14.2+), using
//! `objc2-core-audio` rather than `cidre`. A mono global tap that excludes
//! this process, inside a private aggregate device that holds ONLY the tap
//! (meetily: adding the output device as a sub-device recorded audio twice).
//!
//! THROWAWAY: the IOProc allocates and sends a Vec per callback, which a
//! real-time thread should not do. Production uses a preallocated ring buffer.

use std::ffi::{c_void, CStr};
use std::mem::size_of;
use std::ptr::NonNull;
use std::sync::mpsc::Sender;

use anyhow::{bail, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_audio::*;
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
};
use objc2_core_foundation::{CFDictionary, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

fn check(status: i32, what: &str) -> Result<()> {
    if status != 0 {
        let bytes = status.to_be_bytes();
        let fourcc = if bytes.iter().all(|b| b.is_ascii_graphic()) {
            format!(" '{}'", String::from_utf8_lossy(&bytes))
        } else {
            String::new()
        };
        bail!("{what} failed: OSStatus {status}{fourcc}");
    }
    Ok(())
}

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Reads a fixed-size property, with optional qualifier data.
pub unsafe fn get<T: Copy>(object: u32, selector: u32, qualifier: Option<&[u8]>) -> Result<T> {
    let mut value: T = std::mem::zeroed();
    let mut size = size_of::<T>() as u32;
    let mut addr = address(selector);
    let (qsize, qptr) = qualifier.map_or((0, std::ptr::null()), |q| {
        (q.len() as u32, q.as_ptr() as *const c_void)
    });
    check(
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            qsize,
            qptr,
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        ),
        "AudioObjectGetPropertyData",
    )?;
    Ok(value)
}

/// Reads a CFString property (device UID, tap UID, bundle ID).
pub unsafe fn get_string(object: u32, selector: u32) -> Result<String> {
    let mut value: *const CFString = std::ptr::null();
    let mut size = size_of::<*const CFString>() as u32;
    let mut addr = address(selector);
    check(
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        ),
        "AudioObjectGetPropertyData(string)",
    )?;
    if value.is_null() {
        return Ok(String::new());
    }
    let s = (*value).to_string();
    objc2_core_foundation::CFRetained::from_raw(NonNull::new_unchecked(value as *mut CFString));
    Ok(s)
}

/// Reads an array-of-AudioObjectID property.
pub unsafe fn get_ids(object: u32, selector: u32) -> Result<Vec<u32>> {
    let mut addr = address(selector);
    let mut size = 0u32;
    check(
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        ),
        "AudioObjectGetPropertyDataSize",
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
        "AudioObjectGetPropertyData(ids)",
    )?;
    ids.truncate(size as usize / size_of::<u32>());
    Ok(ids)
}

fn any<T: objc2::Message>(object: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(object) }
}

fn ns(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().expect("ascii key"))
}

pub struct ProcessTap {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    sender: *mut Sender<Vec<f32>>,
    pub sample_rate: f64,
    pub channels: u32,
    pub excluded_self: bool,
}

unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let sender = &*(client as *const Sender<Vec<f32>>);
    let list = input.as_ref();
    let buffers = std::slice::from_raw_parts(
        list.mBuffers.as_ptr() as *const AudioBuffer,
        list.mNumberBuffers as usize,
    );
    for buffer in buffers {
        if buffer.mData.is_null() {
            continue;
        }
        let samples = std::slice::from_raw_parts(
            buffer.mData as *const f32,
            buffer.mDataByteSize as usize / size_of::<f32>(),
        );
        let _ = sender.send(samples.to_vec());
    }
    0
}

impl ProcessTap {
    /// Creates and starts the tap. Samples arrive on `sender` as interleaved
    /// f32 at `sample_rate` with `channels` channels.
    pub fn start(sender: Sender<Vec<f32>>) -> Result<Self> {
        unsafe {
            // This process's audio object exists only once it has used audio,
            // so the caller opens the microphone first.
            let pid = std::process::id() as i32;
            let own: u32 = get(
                kAudioObjectSystemObject as u32,
                kAudioHardwarePropertyTranslatePIDToProcessObject,
                Some(&pid.to_ne_bytes()),
            )
            .unwrap_or(0);
            let excluded: Vec<Retained<NSNumber>> = if own != 0 {
                vec![NSNumber::new_u32(own)]
            } else {
                vec![]
            };
            let excluded = NSArray::from_retained_slice(&excluded);
            let description = CATapDescription::initMonoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &excluded,
            );
            description.setName(&NSString::from_str("ThreeStrands spike tap"));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::Unmuted);

            let mut tap: AudioObjectID = 0;
            check(
                AudioHardwareCreateProcessTap(Some(&description), &mut tap),
                "AudioHardwareCreateProcessTap",
            )?;
            let tap_uid = description.UUID().UUIDString();
            let format: AudioStreamBasicDescription = get(tap, kAudioTapPropertyFormat, None)?;

            let output: u32 = get(
                kAudioObjectSystemObject as u32,
                kAudioHardwarePropertyDefaultSystemOutputDevice,
                None,
            )?;
            let output_uid = get_string(output, kAudioDevicePropertyDeviceUID)?;

            let tap_entry: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_retained_objects(
                    &[
                        &*ns(kAudioSubTapUIDKey),
                        &*ns(kAudioSubTapDriftCompensationKey),
                    ],
                    &[any(tap_uid.clone()), any(NSNumber::new_bool(true))],
                );
            let taps = NSArray::from_retained_slice(&[tap_entry]);
            let aggregate_uid = NSString::from_str(&format!(
                "app.threestrands.spike.tap.{}",
                std::process::id()
            ));
            let description: Retained<NSDictionary<NSString, AnyObject>> =
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
                        any(NSString::from_str("ThreeStrands spike tap")),
                        any(aggregate_uid),
                        any(NSString::from_str(&output_uid)),
                        any(NSNumber::new_bool(true)),
                        any(NSNumber::new_bool(false)),
                        any(NSNumber::new_bool(true)),
                        any(taps),
                    ],
                );
            let cf: &CFDictionary = &*(Retained::as_ptr(&description) as *const CFDictionary);
            let mut aggregate: AudioObjectID = 0;
            if let Err(error) = check(
                AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate)),
                "AudioHardwareCreateAggregateDevice",
            ) {
                AudioHardwareDestroyProcessTap(tap);
                return Err(error);
            }

            let sender = Box::into_raw(Box::new(sender));
            let mut proc_id: AudioDeviceIOProcID = None;
            let created = check(
                AudioDeviceCreateIOProcID(
                    aggregate,
                    Some(io_proc),
                    sender as *mut c_void,
                    NonNull::from(&mut proc_id),
                ),
                "AudioDeviceCreateIOProcID",
            )
            .and_then(|_| check(AudioDeviceStart(aggregate, proc_id), "AudioDeviceStart"));
            if let Err(error) = created {
                AudioHardwareDestroyAggregateDevice(aggregate);
                AudioHardwareDestroyProcessTap(tap);
                drop(Box::from_raw(sender));
                return Err(error);
            }
            Ok(Self {
                tap,
                aggregate,
                proc_id,
                sender,
                sample_rate: format.mSampleRate,
                channels: format.mChannelsPerFrame,
                excluded_self: own != 0,
            })
        }
    }
}

impl Drop for ProcessTap {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.aggregate, self.proc_id);
            AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
            drop(Box::from_raw(self.sender));
        }
    }
}
