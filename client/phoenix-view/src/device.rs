//! What `DeviceInfo.constants` reads off the system: the Android platform
//! reads these off the phone, phoenix-view off Windows.

/// Android's version and API level for the APK's own platform: the base
/// APK of build 356207 is compiled against SDK 36 (`compileSdkVersion`),
/// Android 16 (`platformBuildVersionName`). The real values are the phone's.
pub const OS_VERSION: &str = "16";
pub const API_LEVEL: i32 = 36;

/// A random version 4 UUID, as Java's `UUID.randomUUID()` makes the
/// `vendorId`.
pub fn random_uuid() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut b = [0u8; 16];
    for (i, half) in b.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().fold(String::new(), |mut s, x| {
        use std::fmt::Write as _;
        let _ = write!(s, "{x:02x}");
        s
    });
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// The user's locale as a BCP 47 tag, as `Locale.toLanguageTag()` gives it.
#[cfg(windows)]
pub fn locale() -> String {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    // SAFETY: a buffer of LOCALE_NAME_MAX_LENGTH wide chars and its length.
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 1 {
        return "en-US".into();
    }
    String::from_utf16_lossy(&buf[..(n - 1) as usize])
}

/// Seconds east of UTC now, daylight saving included.
#[cfg(windows)]
pub fn utc_offset_secs() -> i32 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    // SAFETY: an out struct the call fills.
    let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let r = unsafe { GetTimeZoneInformation(&raw mut tz) };
    let bias = tz.Bias
        + match r {
            1 => tz.StandardBias,
            2 => tz.DaylightBias,
            _ => 0,
        };
    -bias * 60
}

/// Total memory in GiB with two decimals, as Java's `ramSize`.
#[cfg(windows)]
pub fn ram_gb() -> String {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: an out struct with its length set, as the call requires.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: as above.
    if unsafe { GlobalMemoryStatusEx(&raw mut m) } == 0 {
        return String::new();
    }
    format!("{:.2}", m.ullTotalPhys as f64 / 1_073_741_824.0)
}

#[cfg(not(windows))]
pub fn locale() -> String {
    "en-US".into()
}

#[cfg(not(windows))]
pub fn utc_offset_secs() -> i32 {
    0
}

#[cfg(not(windows))]
pub fn ram_gb() -> String {
    String::new()
}
