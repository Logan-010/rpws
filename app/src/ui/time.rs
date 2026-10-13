use jiff::{Timestamp, tz::TimeZone};

pub(super) fn format_timestamp(seconds: u64) -> String {
    format_in_zone(seconds, TimeZone::system())
}

fn format_in_zone(seconds: u64, zone: TimeZone) -> String {
    let timestamp = i64::try_from(seconds)
        .ok()
        .and_then(|seconds| Timestamp::from_second(seconds).ok());
    match timestamp {
        Some(timestamp) => timestamp
            .to_zoned(zone)
            .strftime("%Y-%m-%d %H:%M:%S %Z")
            .to_string(),
        None => "Unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_show_timezone_and_handle_invalid_values() {
        assert_eq!(format_in_zone(0, TimeZone::UTC), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_in_zone(u64::MAX, TimeZone::UTC), "Unknown");
        assert_eq!(format_in_zone(i64::MAX as u64, TimeZone::UTC), "Unknown");
        let zone = TimeZone::get("America/New_York").unwrap();
        let winter: Timestamp = "2026-01-15T12:34:56Z".parse().unwrap();
        let summer: Timestamp = "2026-07-15T12:34:56Z".parse().unwrap();
        assert_eq!(
            format_in_zone(winter.as_second() as u64, zone.clone()),
            "2026-01-15 07:34:56 EST"
        );
        assert_eq!(
            format_in_zone(summer.as_second() as u64, zone),
            "2026-07-15 08:34:56 EDT"
        );
    }
}
