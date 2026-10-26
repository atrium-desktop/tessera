//! IPv4 neighbour-table lookup: turning a peer's device address into the address a
//! socket can reach.
//!
//! A Wi-Fi Display sink is discovered by its **device address** (a MAC), but RTSP
//! needs an IP. On a formed P2P group the sink has an address from the group's
//! DHCP, and the kernel's neighbour table is where it becomes visible
//! (`/proc/net/arp`).
//!
//! Three rules make this honest rather than hopeful:
//!
//! - An **incomplete** entry (flags without `ATF_COM`) is not an address. The
//!   kernel lists a row before it has resolved one, and reporting it would send a
//!   socket at a MAC.
//! - A MAC with several rows is **ambiguous**, not "pick the first". Two
//!   interfaces can hold the same peer.
//! - Absence is reported as absence. The fix is to let the group settle (or ping
//!   the sink once), not to guess a subnet.

use std::fmt;
use std::net::Ipv4Addr;

/// `ATF_COM` from `linux/if_arp.h`: the entry has a usable hardware address.
const ATF_COM: u32 = 0x02;

/// One row of the neighbour table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neighbour {
    pub ip: Ipv4Addr,
    pub mac: [u8; 6],
    /// The `Flags` column, verbatim.
    pub flags: u32,
    /// The interface the row belongs to.
    pub device: String,
}

impl Neighbour {
    /// Whether the kernel has actually resolved this row.
    pub fn is_complete(&self) -> bool {
        self.flags & ATF_COM != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeighbourError {
    /// The table could not be read at all.
    Unreadable(String),
    /// A row was malformed.
    BadRow(String),
}

impl fmt::Display for NeighbourError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(reason) => write!(formatter, "neighbour table unreadable: {reason}"),
            Self::BadRow(row) => write!(formatter, "malformed neighbour row: {row:?}"),
        }
    }
}

impl std::error::Error for NeighbourError {}

/// What the table says about one device address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one complete row.
    Found(Ipv4Addr),
    /// Rows exist but none is complete: the kernel has not resolved the peer yet.
    Incomplete,
    /// No row at all.
    NotPresent,
    /// Several complete rows; the caller must choose.
    Ambiguous(Vec<Ipv4Addr>),
}

/// Parse `/proc/net/arp`.
///
/// The header line is skipped; blank lines are ignored. A row whose first field is
/// not an IPv4 address is a parse error rather than a silently dropped entry: a
/// table we cannot read correctly must not be treated as "peer absent".
pub fn parse_arp(text: &str) -> Result<Vec<Neighbour>, NeighbourError> {
    let mut neighbours = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        // The column header is skipped by content, not by line number: a table
        // with a leading blank line would otherwise fail on its own header.
        if fields.first() == Some(&"IP") {
            continue;
        }
        if fields.len() < 6 {
            return Err(NeighbourError::BadRow(trimmed.to_owned()));
        }
        let ip = fields[0]
            .parse::<Ipv4Addr>()
            .map_err(|_| NeighbourError::BadRow(trimmed.to_owned()))?;
        let mac = parse_mac(fields[3]).ok_or_else(|| NeighbourError::BadRow(trimmed.to_owned()))?;
        let flags = u32::from_str_radix(fields[2].trim_start_matches("0x"), 16)
            .map_err(|_| NeighbourError::BadRow(trimmed.to_owned()))?;
        neighbours.push(Neighbour {
            ip,
            mac,
            flags,
            device: fields[5].to_owned(),
        });
    }
    Ok(neighbours)
}

/// Parse a colon-separated MAC, case-insensitively.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let mut parts = text.split(':');
    for slot in &mut mac {
        let part = parts.next()?;
        if part.len() != 2 {
            return None;
        }
        *slot = u8::from_str_radix(part, 16).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(mac)
}

/// Canonical lower-case colon form, so a MAC from iwd and a MAC from the kernel
/// compare equal regardless of how either formats it.
pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Resolve one device address against the table.
///
/// `device` optionally restricts the search to one interface, which matters when a
/// peer is reachable on both the station link and a P2P group: the group interface
/// is the one whose address the sink will answer on.
pub fn resolve(mac: &[u8; 6], table: &[Neighbour], device: Option<&str>) -> Resolution {
    let rows: Vec<&Neighbour> = table
        .iter()
        .filter(|row| &row.mac == mac)
        .filter(|row| device.is_none_or(|device| row.device == device))
        .collect();
    if rows.is_empty() {
        return Resolution::NotPresent;
    }
    let mut complete: Vec<Ipv4Addr> = rows
        .iter()
        .filter(|row| row.is_complete())
        .map(|row| row.ip)
        .collect();
    complete.sort_unstable();
    complete.dedup();
    match complete.len() {
        0 => Resolution::Incomplete,
        1 => Resolution::Found(complete[0]),
        _ => Resolution::Ambiguous(complete),
    }
}

/// Read the live neighbour table.
pub fn read_arp() -> Result<Vec<Neighbour>, NeighbourError> {
    let text = std::fs::read_to_string("/proc/net/arp")
        .map_err(|error| NeighbourError::Unreadable(error.to_string()))?;
    parse_arp(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
10.117.143.108   0x1         0x2         4e:46:20:9a:0b:1d     *        wlan0
192.168.49.1     0x1         0x2         0A:11:22:33:44:55     *        p2p-wlan0-0
192.168.49.7     0x1         0x0         6a:77:88:99:aa:bb     *        p2p-wlan0-0
";

    fn table() -> Vec<Neighbour> {
        parse_arp(SAMPLE).expect("parse")
    }

    #[test]
    fn the_header_and_blank_lines_are_skipped() {
        let table = parse_arp(&format!("\n{SAMPLE}\n")).expect("parse");
        assert_eq!(table.len(), 3);
        assert_eq!(table[0].ip, Ipv4Addr::new(10, 117, 143, 108));
        assert_eq!(table[0].device, "wlan0");
    }

    #[test]
    fn a_complete_row_resolves() {
        let mac = parse_mac("0a:11:22:33:44:55").expect("mac");
        assert_eq!(
            resolve(&mac, &table(), None),
            Resolution::Found(Ipv4Addr::new(192, 168, 49, 1))
        );
    }

    #[test]
    fn an_incomplete_row_is_not_an_address() {
        // Flags 0x0 means the kernel has a row but no resolved hardware address.
        // Skipping this check would aim a socket at a MAC.
        let mac = parse_mac("6a:77:88:99:aa:bb").expect("mac");
        assert_eq!(resolve(&mac, &table(), None), Resolution::Incomplete);
    }

    #[test]
    fn an_absent_mac_is_reported_as_absent() {
        let mac = parse_mac("00:00:00:00:00:01").expect("mac");
        assert_eq!(resolve(&mac, &table(), None), Resolution::NotPresent);
    }

    #[test]
    fn the_device_filter_picks_the_right_interface() {
        let table = vec![
            Neighbour {
                ip: Ipv4Addr::new(10, 0, 0, 5),
                mac: [1, 2, 3, 4, 5, 6],
                flags: ATF_COM,
                device: "wlan0".to_owned(),
            },
            Neighbour {
                ip: Ipv4Addr::new(192, 168, 49, 5),
                mac: [1, 2, 3, 4, 5, 6],
                flags: ATF_COM,
                device: "p2p-wlan0-0".to_owned(),
            },
        ];
        let mac = [1, 2, 3, 4, 5, 6];
        assert_eq!(
            resolve(&mac, &table, Some("p2p-wlan0-0")),
            Resolution::Found(Ipv4Addr::new(192, 168, 49, 5)),
            "the peer on the P2P interface is the one a cast can reach"
        );
        assert_eq!(
            resolve(&mac, &table, None),
            Resolution::Ambiguous(vec![
                Ipv4Addr::new(10, 0, 0, 5),
                Ipv4Addr::new(192, 168, 49, 5)
            ]),
            "without a hint, two interfaces are ambiguous rather than a coin flip"
        );
    }

    #[test]
    fn macs_compare_case_insensitively_and_format_canonically() {
        let lower = parse_mac("4e:46:20:9a:0b:1d").expect("mac");
        let upper = parse_mac("4E:46:20:9A:0B:1D").expect("mac");
        assert_eq!(lower, upper);
        assert_eq!(format_mac(&upper), "4e:46:20:9a:0b:1d");
        // iwd echoes the kernel's MAC back in either case; the lookup must match.
        assert_eq!(
            resolve(&upper, &table(), None),
            Resolution::Found(Ipv4Addr::new(10, 117, 143, 108))
        );
    }

    #[test]
    fn a_malformed_mac_is_rejected_rather_than_zero_filled() {
        assert_eq!(parse_mac("4e:46:20:9a:0b"), None, "too few octets");
        assert_eq!(parse_mac("4e:46:20:9a:0b:1d:00"), None, "too many octets");
        assert_eq!(parse_mac("4e-46-20-9a-0b-1d"), None, "wrong separator");
        assert_eq!(parse_mac("zz:46:20:9a:0b:1d"), None, "not hex");
        assert_eq!(parse_mac("4e:46:20:9a:0b:1de"), None, "octet too long");
    }

    #[test]
    fn a_malformed_row_is_an_error_not_a_silent_drop() {
        // Dropping the row would be indistinguishable from "peer absent".
        let malformed = "IP address       HW type     Flags       HW address            Mask     Device\n\
                         not-an-ip        0x1         0x2         aa:bb:cc:dd:ee:ff     *        wlan0\n";
        assert!(matches!(
            parse_arp(malformed),
            Err(NeighbourError::BadRow(_))
        ));
        let short = "IP address       HW type     Flags\n00:11\n";
        assert!(matches!(parse_arp(short), Err(NeighbourError::BadRow(_))));
    }

    #[test]
    fn the_live_table_parses_on_this_machine() {
        // A read-only sanity check against the real kernel table when it exists;
        // a sandbox without one skips rather than failing.
        match read_arp() {
            Ok(table) => {
                for row in &table {
                    assert_eq!(format_mac(&row.mac).len(), 17);
                    assert!(!row.device.is_empty());
                }
            }
            Err(NeighbourError::Unreadable(reason)) => {
                eprintln!("skipping: neighbour table unreadable: {reason}");
            }
            Err(error) => panic!("the live table must parse: {error}"),
        }
    }
}
