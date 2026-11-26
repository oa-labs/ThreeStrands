//! Meeting-detection probe: is the default input running somewhere, and
//! which processes are using audio input or output right now.
//!
//!   probe [seconds]   (polls once a second)

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use meeting_recording_spike::tap::{get, get_ids, get_string};
    use objc2_core_audio::*;

    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let system = kAudioObjectSystemObject as u32;
    for tick in 0..seconds {
        unsafe {
            let input: u32 = get(system, kAudioHardwarePropertyDefaultInputDevice, None)?;
            let running: u32 = get(input, kAudioDevicePropertyDeviceIsRunningSomewhere, None)?;
            println!(
                "t={tick}s default input {input} ({}) running_somewhere={running}",
                get_string(input, kAudioDevicePropertyDeviceUID)?
            );
            for process in get_ids(system, kAudioHardwarePropertyProcessObjectList)? {
                let pid: i32 = get(process, kAudioProcessPropertyPID, None).unwrap_or(-1);
                let bundle = get_string(process, kAudioProcessPropertyBundleID).unwrap_or_default();
                let input: u32 =
                    get(process, kAudioProcessPropertyIsRunningInput, None).unwrap_or(0);
                let output: u32 =
                    get(process, kAudioProcessPropertyIsRunningOutput, None).unwrap_or(0);
                if input != 0 || output != 0 {
                    println!("  pid {pid:>6} {bundle:<40} input={input} output={output}");
                }
            }
        }
        if tick + 1 < seconds {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("probe is macOS-only");
}
