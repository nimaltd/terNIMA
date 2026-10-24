//! Debug probes: which are plugged in, the chips probe-rs knows, and attaching
//! to a chip. RTT and SWO sessions both start here.

use std::sync::OnceLock;

use probe_rs::config::Registry;
use probe_rs::probe::list::Lister;
use probe_rs::{Permissions, Session};
use serde::{Deserialize, Serialize};

/// What identifies a debug probe, so the same one is found again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeId {
    #[serde(with = "crate::hex::u16")]
    pub vid: u16,
    #[serde(with = "crate::hex::u16")]
    pub pid: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

/// One debug probe plugged in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProbeInfo {
    /// What probe-rs calls it, such as "STLink V3" or "CMSIS-DAP".
    pub name: String,
    pub id: ProbeId,
}

/// How to reach a chip: through which probe, and how to attach.
pub struct Target<'a> {
    /// Without a probe, the only one plugged in is used.
    pub probe: Option<&'a ProbeId>,
    /// As probe-rs names it, such as "STM32H503RB".
    pub chip: &'a str,
    /// The SWD clock in kHz, or 0 for the probe's own default.
    pub speed_khz: u32,
    /// Holds the chip in reset while attaching. This resets the chip.
    pub under_reset: bool,
}

pub fn list_probes() -> Vec<ProbeInfo> {
    Lister::new()
        .list_all()
        .into_iter()
        .map(|probe| ProbeInfo {
            name: probe.identifier.clone(),
            id: ProbeId {
                vid: probe.vendor_id,
                pid: probe.product_id,
                serial: probe.serial_number.filter(|serial| !serial.is_empty()),
            },
        })
        .collect()
}

/// Opens the probe and attaches to the chip, without halting it. Gives
/// `Err(None)` when the probe is not plugged in, and the reason when it is
/// but did not work.
pub fn attach(target: &Target) -> Result<Session, Option<String>> {
    let probes = Lister::new().list_all();
    let info = match target.probe {
        Some(id) => probes.iter().find(|probe| {
            probe.vendor_id == id.vid
                && probe.product_id == id.pid
                && (id.serial.is_none() || probe.serial_number == id.serial)
        }),
        None => match probes.as_slice() {
            [only] => Some(only),
            [] => None,
            _ => {
                return Err(Some(
                    "More than one probe is plugged in: choose one.".into(),
                ));
            }
        },
    }
    .ok_or(None)?;

    let mut probe = info.open().map_err(|error| {
        Some(format!(
            "Could not open {}: {error}. If a debugger has it open, close that first.",
            info.identifier
        ))
    })?;
    if target.speed_khz > 0 {
        probe.set_speed(target.speed_khz).map_err(|error| {
            Some(format!(
                "Could not set the speed to {} kHz: {error}",
                target.speed_khz
            ))
        })?;
    }
    let chip = target.chip.trim();
    let attached = if target.under_reset {
        probe.attach_under_reset(chip, Permissions::default())
    } else {
        probe.attach(chip, Permissions::default())
    };
    attached.map_err(|error| Some(format!("Could not attach to {chip}: {error}")))
}

/// Every chip name probe-rs knows, read once.
fn chips() -> &'static [String] {
    static CHIPS: OnceLock<Vec<String>> = OnceLock::new();
    CHIPS.get_or_init(|| {
        let mut names: Vec<String> = Registry::from_builtin_families()
            .families()
            .iter()
            .flat_map(|family| family.variants.iter().map(|chip| chip.name.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    })
}

/// Chips whose name holds `text`, ignoring case: those that start with it first.
pub fn search_chips(text: &str, limit: usize) -> Vec<String> {
    let wanted = text.trim().to_uppercase();
    if wanted.is_empty() {
        return Vec::new();
    }
    let (mut starts, mut contains): (Vec<&String>, Vec<&String>) = chips()
        .iter()
        .filter(|name| name.to_uppercase().contains(&wanted))
        .partition(|name| name.to_uppercase().starts_with(&wanted));
    starts.append(&mut contains);
    starts.into_iter().take(limit).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_chips_by_any_part_of_the_name() {
        let found = search_chips("h503rb", 10);
        assert!(found.contains(&"STM32H503RB".to_string()), "{found:?}");
        let found = search_chips("nrf52840", 5);
        assert!(
            found
                .iter()
                .all(|name| name.to_uppercase().contains("NRF52840")),
            "{found:?}"
        );
        assert!(!found.is_empty());
    }

    #[test]
    fn names_that_start_with_the_text_come_first() {
        let found = search_chips("STM32F4", 50);
        assert!(
            found
                .first()
                .is_some_and(|name| name.starts_with("STM32F4")),
            "{found:?}"
        );
    }

    #[test]
    fn empty_search_finds_nothing() {
        assert!(search_chips("  ", 10).is_empty());
    }

    #[test]
    fn probe_ids_are_hex_text() {
        let id = ProbeId {
            vid: 0x0483,
            pid: 0x3754,
            serial: None,
        };
        let text = toml::to_string(&id).unwrap();
        assert_eq!(text, "vid = \"0483\"\npid = \"3754\"\n");
    }
}
