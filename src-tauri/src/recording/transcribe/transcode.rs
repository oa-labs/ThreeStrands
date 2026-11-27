//! Post-transcription audio compression and retry decoding on macOS via
//! `AudioToolbox`'s `ExtAudioFile` API (zero new crates).
//!
//! Live capture writes crash-safe 60-second 16 kHz mono 16-bit PCM WAV files.
//! Once transcription succeeds (and **Keep Audio** is not `Until Transcribed`),
//! [`transcode_recording_dir`] compresses each WAV segment to AAC/M4A (~7–8×
//! smaller) and updates the track manifests with the exact `audioMs` length so
//! **Retry** and usage accounting still work seamlessly.

use std::path::Path;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::recording::capture::Track;
    use crate::recording::files;
    use crate::recording::policy;
    use crate::recording::segments::{
        manifest_path, read_manifest, segment_file_name, transcoded_file_name, ManifestEntry,
    };
    use objc2_core_audio_types::{
        kAudioFormatFlagIsFloat, kAudioFormatLinearPCM, AudioBuffer, AudioBufferList,
        AudioStreamBasicDescription,
    };
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::unix::ffi::OsStrExt;

    type ExtAudioFileRef = *mut c_void;
    type CFAllocatorRef = *const c_void;
    type CFURLRef = *const c_void;

    const K_CF_URL_POSIX_PATH_STYLE: isize = 0;
    const K_AUDIO_FILE_M4A_TYPE: u32 = u32::from_be_bytes(*b"m4af");
    const K_AUDIO_FORMAT_MPEG4_AAC: u32 = u32::from_be_bytes(*b"aac ");
    const K_AUDIO_FILE_FLAGS_ERASE_FILE: u32 = 1;
    const K_EXT_AUDIO_FILE_PROPERTY_CLIENT_DATA_FORMAT: u32 = u32::from_be_bytes(*b"cfmt");
    const K_EXT_AUDIO_FILE_PROPERTY_FILE_LENGTH_FRAMES: u32 = u32::from_be_bytes(*b"#frm");
    const K_EXT_AUDIO_FILE_PROPERTY_FILE_DATA_FORMAT: u32 = u32::from_be_bytes(*b"ffmt");
    const K_AUDIO_FORMAT_FLAG_IS_SIGNED_INTEGER: u32 = 1 << 2;
    const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;
    const READ_CHUNK_FRAMES: u32 = 4096;

    #[link(name = "AudioToolbox", kind = "framework")]
    extern "C" {
        fn ExtAudioFileCreateWithURL(
            in_url: CFURLRef,
            in_file_type: u32,
            in_stream_desc: *const AudioStreamBasicDescription,
            in_channel_layout: *const c_void,
            in_flags: u32,
            out_ext_audio_file: *mut ExtAudioFileRef,
        ) -> i32;
        fn ExtAudioFileOpenURL(in_url: CFURLRef, out_ext_audio_file: *mut ExtAudioFileRef) -> i32;
        fn ExtAudioFileDispose(in_ext_audio_file: ExtAudioFileRef) -> i32;
        fn ExtAudioFileSetProperty(
            in_ext_audio_file: ExtAudioFileRef,
            in_property_id: u32,
            in_property_data_size: u32,
            in_property_data: *const c_void,
        ) -> i32;
        fn ExtAudioFileGetProperty(
            in_ext_audio_file: ExtAudioFileRef,
            in_property_id: u32,
            io_property_data_size: *mut u32,
            out_property_data: *mut c_void,
        ) -> i32;
        fn ExtAudioFileRead(
            in_ext_audio_file: ExtAudioFileRef,
            io_number_frames: *mut u32,
            io_data: *mut AudioBufferList,
        ) -> i32;
        fn ExtAudioFileWrite(
            in_ext_audio_file: ExtAudioFileRef,
            in_number_frames: u32,
            io_data: *const AudioBufferList,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFAllocatorDefault: CFAllocatorRef;
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFAllocatorRef,
            buffer: *const u8,
            buf_len: isize,
            is_directory: u8,
        ) -> CFURLRef;
        fn CFRelease(cf: *const c_void);
    }

    struct CfUrl(CFURLRef);
    impl CfUrl {
        fn from_path(path: &Path) -> Result<Self, String> {
            let bytes = path.as_os_str().as_bytes();
            let ptr = unsafe {
                CFURLCreateFromFileSystemRepresentation(
                    kCFAllocatorDefault,
                    bytes.as_ptr(),
                    bytes.len() as isize,
                    0,
                )
            };
            let _ = K_CF_URL_POSIX_PATH_STYLE;
            if ptr.is_null() {
                return Err(format!(
                    "Invalid path for CoreFoundation URL: {}",
                    path.display()
                ));
            }
            Ok(Self(ptr))
        }
    }
    impl Drop for CfUrl {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    struct ExtAudioFile(ExtAudioFileRef);
    impl Drop for ExtAudioFile {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { ExtAudioFileDispose(self.0) };
            }
        }
    }

    fn status_err(status: i32, what: &str) -> Result<(), String> {
        if status == 0 {
            Ok(())
        } else {
            Err(format!("{what} failed (OSStatus {status})"))
        }
    }

    fn pcm_i16_asbd() -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            mSampleRate: policy::OUTPUT_SAMPLE_RATE as f64,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: K_AUDIO_FORMAT_FLAG_IS_SIGNED_INTEGER | K_AUDIO_FORMAT_FLAG_IS_PACKED,
            mBytesPerPacket: 2,
            mFramesPerPacket: 1,
            mBytesPerFrame: 2,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 16,
            mReserved: 0,
        }
    }

    fn pcm_f32_asbd() -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            mSampleRate: policy::OUTPUT_SAMPLE_RATE as f64,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | K_AUDIO_FORMAT_FLAG_IS_PACKED,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 32,
            mReserved: 0,
        }
    }

    fn aac_asbd() -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            mSampleRate: policy::OUTPUT_SAMPLE_RATE as f64,
            mFormatID: K_AUDIO_FORMAT_MPEG4_AAC,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 1024,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 0,
            mReserved: 0,
        }
    }

    /// Compresses a 16 kHz mono 16-bit PCM WAV segment into AAC/M4A at `m4a_path`.
    pub fn transcode_wav_to_m4a(wav_path: &Path, m4a_path: &Path) -> Result<u64, String> {
        let file =
            File::open(wav_path).map_err(|e| format!("opening {}: {e}", wav_path.display()))?;
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        if length > policy::WORKER_MAX_SEGMENT_FILE_BYTES {
            return Err("the WAV segment is too large".into());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(policy::WORKER_MAX_SEGMENT_FILE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() < 44 {
            return Err("the WAV segment has no header".into());
        }
        let expected = crate::recording::segments::wav_header(0);
        if bytes[0..4] != expected[0..4] || bytes[8..40] != expected[8..40] {
            return Err("the WAV segment isn't 16 kHz mono 16-bit audio".into());
        }
        let pcm = &bytes[44..];
        let total_frames = (pcm.len() / 2) as u64;
        let audio_ms = total_frames * 1000 / policy::OUTPUT_SAMPLE_RATE as u64;

        let tmp_path = m4a_path.with_extension("m4a.tmp");
        let _ = std::fs::remove_file(&tmp_path);
        let url = CfUrl::from_path(&tmp_path)?;
        let out_desc = aac_asbd();
        let mut raw_ref: ExtAudioFileRef = std::ptr::null_mut();
        status_err(
            unsafe {
                ExtAudioFileCreateWithURL(
                    url.0,
                    K_AUDIO_FILE_M4A_TYPE,
                    &out_desc,
                    std::ptr::null(),
                    K_AUDIO_FILE_FLAGS_ERASE_FILE,
                    &mut raw_ref,
                )
            },
            "creating M4A file",
        )?;
        let ext = ExtAudioFile(raw_ref);
        let client = pcm_i16_asbd();
        status_err(
            unsafe {
                ExtAudioFileSetProperty(
                    ext.0,
                    K_EXT_AUDIO_FILE_PROPERTY_CLIENT_DATA_FORMAT,
                    std::mem::size_of::<AudioStreamBasicDescription>() as u32,
                    (&client as *const AudioStreamBasicDescription).cast(),
                )
            },
            "setting client PCM format",
        )?;

        let mut offset = 0usize;
        let chunk_bytes = READ_CHUNK_FRAMES as usize * 2;
        while offset < pcm.len() {
            let end = (offset + chunk_bytes).min(pcm.len());
            let byte_len = ((end - offset) & !1) as u32;
            if byte_len == 0 {
                break;
            }
            let frames = byte_len / 2;
            let list = AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: byte_len,
                    mData: pcm[offset..offset + byte_len as usize].as_ptr() as *mut c_void,
                }],
            };
            status_err(
                unsafe { ExtAudioFileWrite(ext.0, frames, &list) },
                "writing AAC frames",
            )?;
            offset += byte_len as usize;
        }
        drop(ext);
        files::restrict_file_to_owner(&tmp_path).map_err(|e| e.to_string())?;
        files::exclude_from_backup(&tmp_path);
        std::fs::rename(&tmp_path, m4a_path).map_err(|e| e.to_string())?;
        Ok(audio_ms)
    }

    /// Decodes an M4A segment back into 16 kHz mono f32 samples in `-1..1`.
    pub fn read_m4a_samples(path: &Path) -> Result<Vec<f32>, String> {
        let url = CfUrl::from_path(path)?;
        let mut raw_ref: ExtAudioFileRef = std::ptr::null_mut();
        status_err(
            unsafe { ExtAudioFileOpenURL(url.0, &mut raw_ref) },
            "opening M4A file",
        )?;
        let ext = ExtAudioFile(raw_ref);
        let client = pcm_f32_asbd();
        status_err(
            unsafe {
                ExtAudioFileSetProperty(
                    ext.0,
                    K_EXT_AUDIO_FILE_PROPERTY_CLIENT_DATA_FORMAT,
                    std::mem::size_of::<AudioStreamBasicDescription>() as u32,
                    (&client as *const AudioStreamBasicDescription).cast(),
                )
            },
            "setting M4A client format",
        )?;
        let max_samples = (policy::WORKER_MAX_SEGMENT_FILE_BYTES / 2) as usize;
        let mut out = Vec::new();
        let mut buf = vec![0f32; READ_CHUNK_FRAMES as usize];
        loop {
            let mut frames = READ_CHUNK_FRAMES;
            let mut list = AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: (buf.len() * std::mem::size_of::<f32>()) as u32,
                    mData: buf.as_mut_ptr().cast(),
                }],
            };
            status_err(
                unsafe { ExtAudioFileRead(ext.0, &mut frames, &mut list) },
                "reading M4A samples",
            )?;
            if frames == 0 {
                break;
            }
            if out.len() + frames as usize > max_samples {
                return Err("the M4A segment is too large".into());
            }
            out.extend_from_slice(&buf[..frames as usize]);
        }
        Ok(out)
    }

    /// Reads an M4A file's duration in milliseconds using its header metadata.
    pub fn m4a_duration_ms(path: &Path) -> Option<u64> {
        let url = CfUrl::from_path(path).ok()?;
        let mut raw_ref: ExtAudioFileRef = std::ptr::null_mut();
        if unsafe { ExtAudioFileOpenURL(url.0, &mut raw_ref) } != 0 {
            return None;
        }
        let ext = ExtAudioFile(raw_ref);
        let mut file_desc = AudioStreamBasicDescription {
            mSampleRate: 0.0,
            mFormatID: 0,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 0,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 0,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        let mut desc_size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        if unsafe {
            ExtAudioFileGetProperty(
                ext.0,
                K_EXT_AUDIO_FILE_PROPERTY_FILE_DATA_FORMAT,
                &mut desc_size,
                (&mut file_desc as *mut AudioStreamBasicDescription).cast(),
            )
        } != 0
            || file_desc.mSampleRate <= 0.0
        {
            return None;
        }
        let mut frames: i64 = 0;
        let mut frames_size = std::mem::size_of::<i64>() as u32;
        if unsafe {
            ExtAudioFileGetProperty(
                ext.0,
                K_EXT_AUDIO_FILE_PROPERTY_FILE_LENGTH_FRAMES,
                &mut frames_size,
                (&mut frames as *mut i64).cast(),
            )
        } != 0
            || frames <= 0
        {
            return None;
        }
        Some(((frames as f64 / file_desc.mSampleRate) * 1000.0).round() as u64)
    }

    /// Transcodes all `.wav` segment files in `audio_dir` to AAC `.m4a`,
    /// updates each track manifest atomically with `audioMs`, and deletes the
    /// source `.wav` files. Returns the count of transcoded segments.
    pub fn transcode_recording_dir(audio_dir: &Path) -> Result<usize, String> {
        let mut count = 0usize;
        for track in Track::ALL {
            let entries = read_manifest(audio_dir, track);
            if entries.is_empty() {
                continue;
            }
            let mut updated = Vec::with_capacity(entries.len());
            let mut to_remove = Vec::new();
            let mut changed = false;
            for entry in entries {
                match entry {
                    ManifestEntry::Segment {
                        segment,
                        file,
                        start_ms,
                        audio_ms,
                    } if file == segment_file_name(track, segment) => {
                        let wav_path = audio_dir.join(&file);
                        let m4a_name = transcoded_file_name(track, segment);
                        let m4a_path = audio_dir.join(&m4a_name);
                        if wav_path.is_file() {
                            let ms = transcode_wav_to_m4a(&wav_path, &m4a_path)?;
                            to_remove.push(wav_path);
                            count += 1;
                            changed = true;
                            updated.push(ManifestEntry::Segment {
                                segment,
                                file: m4a_name,
                                start_ms,
                                audio_ms: Some(ms),
                            });
                        } else {
                            updated.push(ManifestEntry::Segment {
                                segment,
                                file,
                                start_ms,
                                audio_ms,
                            });
                        }
                    }
                    other => updated.push(other),
                }
            }
            if changed {
                let manifest = manifest_path(audio_dir, track);
                let tmp = manifest.with_extension("jsonl.tmp");
                let mut f = files::create_private_file(&tmp).map_err(|e| e.to_string())?;
                for entry in &updated {
                    let mut line = serde_json::to_vec(entry).map_err(|e| e.to_string())?;
                    line.push(b'\n');
                    f.write_all(&line).map_err(|e| e.to_string())?;
                }
                f.sync_data().map_err(|e| e.to_string())?;
                drop(f);
                std::fs::rename(&tmp, &manifest).map_err(|e| e.to_string())?;
                for wav in to_remove {
                    let _ = std::fs::remove_file(wav);
                }
            }
        }
        Ok(count)
    }
}

#[cfg(not(target_os = "macos"))]
mod non_mac {
    use std::path::Path;

    pub fn read_m4a_samples(_path: &Path) -> Result<Vec<f32>, String> {
        Err("M4A segment decoding isn't available on this platform.".into())
    }

    pub fn m4a_duration_ms(_path: &Path) -> Option<u64> {
        None
    }

    pub fn transcode_recording_dir(_audio_dir: &Path) -> Result<usize, String> {
        Ok(0)
    }
}

#[cfg(target_os = "macos")]
pub use mac::{m4a_duration_ms, read_m4a_samples, transcode_recording_dir};
#[cfg(not(target_os = "macos"))]
pub use non_mac::{m4a_duration_ms, read_m4a_samples, transcode_recording_dir};

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::recording::capture::Track;
    use crate::recording::segments::{read_manifest, ManifestEntry, SegmentWriter};
    use crate::recording::test_support::TempDir;

    #[test]
    fn transcode_compresses_wav_to_m4a_and_decodes_round_trip() {
        let dir = TempDir::new();
        let mut writer = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        let samples: Vec<i16> = (0..16_000 * 10)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 12_000.0) as i16
            })
            .collect();
        writer.write(&samples, 0).unwrap();
        drop(writer);

        let wav_len = std::fs::metadata(dir.path().join("mic-0001.wav"))
            .unwrap()
            .len();
        let count = transcode_recording_dir(dir.path()).unwrap();
        assert_eq!(count, 1);
        assert!(!dir.path().join("mic-0001.wav").exists());
        let m4a_path = dir.path().join("mic-0001.m4a");
        assert!(m4a_path.is_file());
        let m4a_len = std::fs::metadata(&m4a_path).unwrap().len();
        assert!(
            m4a_len < wav_len / 3,
            "expected M4A ({m4a_len}) to be much smaller than WAV ({wav_len})"
        );
        let decoded = read_m4a_samples(&m4a_path).unwrap();
        assert!(
            (158_000..=162_000).contains(&decoded.len()),
            "decoded {} samples",
            decoded.len()
        );
        let peak = decoded.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.1, "decoded audio shouldn't be silent ({peak})");

        let manifest = read_manifest(dir.path(), Track::Mic);
        assert!(matches!(
            manifest.first(),
            Some(ManifestEntry::Segment {
                file,
                audio_ms: Some(10_000),
                ..
            }) if file == "mic-0001.m4a"
        ));
    }
}
