//! Finding out which chip is on the board.
//!
//! probe-rs finds some chips by itself (Nordic, Microchip SAM, Espressif and
//! others) but cannot tell STM32 chips apart. Those are found by reading their
//! CPUID and DBGMCU ID registers, only reading, so the firmware is not touched.

use probe_rs::Permissions;
use probe_rs::architecture::arm::FullyQualifiedApAddress;
use probe_rs::architecture::arm::sequences::DefaultArmSequence;
use probe_rs::config::TargetSelector;
use serde::Serialize;

use crate::probe::{self, ProbeId, search_chips};

/// What was found on the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Detected {
    /// The chip to use, when one is likely enough.
    pub chip: Option<String>,
    /// Every chip it may be, for the user to pick the exact one: the same
    /// silicon often comes in several packages and flash sizes.
    pub candidates: Vec<String>,
    /// What was found, in words, such as "STM32H503 (Cortex-M33, DEV_ID 0x474)".
    pub description: String,
}

/// Looks at the chip through the probe. STM32 chips are read first, as
/// probe-rs's own detection does not know them.
pub fn detect(probe: Option<&ProbeId>, speed_khz: u32) -> Result<Detected, String> {
    if let Some(found) = read_stm32(probe, speed_khz)? {
        return Ok(found);
    }
    let opened = probe::open(probe, speed_khz)?;
    match opened.attach(TargetSelector::Auto, Permissions::default()) {
        Ok(session) => {
            let name = session.target().name.clone();
            Ok(Detected {
                chip: Some(name.clone()),
                candidates: vec![name.clone()],
                description: name,
            })
        }
        Err(_) => Err(
            "The chip could not be found out. Type its name: probe-rs knows most chips by name."
                .into(),
        ),
    }
}

/// Reads an STM32's CPUID and DBGMCU_IDCODE, or gives `None` for another chip.
fn read_stm32(probe: Option<&ProbeId>, speed_khz: u32) -> Result<Option<Detected>, String> {
    let mut opened = probe::open(probe, speed_khz)?;
    opened
        .attach_to_unspecified()
        .map_err(|error| format!("Could not reach the chip: {error}"))?;
    let mut interface = opened
        .try_into_arm_debug_interface(DefaultArmSequence::create())
        .map_err(|(_, error)| format!("Could not reach the chip: {error}"))?;

    // The core sits on access port 0 on most chips and 1 on some, such as the STM32H5.
    for ap in [0, 1] {
        let Ok(mut memory) =
            interface.memory_interface(&FullyQualifiedApAddress::v1_with_default_dp(ap))
        else {
            continue;
        };
        let Ok(cpuid) = memory.read_word_32(CPUID) else {
            continue;
        };
        let Some(core) = core_name(cpuid) else {
            continue;
        };
        for &address in idcode_addresses(cpuid) {
            let Ok(idcode) = memory.read_word_32(address) else {
                continue;
            };
            let dev_id = (idcode & 0xFFF) as u16;
            if dev_id == 0 {
                continue;
            }
            return Ok(Some(stm32(dev_id, core)));
        }
    }
    Ok(None)
}

/// What an STM32's DEV_ID says, with the chips probe-rs knows by those names.
fn stm32(dev_id: u16, core: &str) -> Detected {
    let Some(families) = stm32_families(dev_id) else {
        return Detected {
            chip: None,
            candidates: Vec::new(),
            description: format!(
                "an STM32 with DEV_ID {dev_id:#05x} ({core}) that ternimal does not know yet"
            ),
        };
    };
    let candidates: Vec<String> = families
        .iter()
        .flat_map(|family| search_chips(family, 100))
        .filter(|chip| families.iter().any(|family| chip.starts_with(family)))
        .collect();
    Detected {
        chip: candidates.first().cloned(),
        description: format!("{} ({core}, DEV_ID {dev_id:#05x})", families.join(", ")),
        candidates,
    }
}

const CPUID: u64 = 0xE000_ED00;

/// The core, from the part number in CPUID.
fn core_name(cpuid: u32) -> Option<&'static str> {
    // ARM is implementer 0x41.
    if cpuid >> 24 != 0x41 {
        return None;
    }
    Some(match (cpuid >> 4) & 0xFFF {
        0xC20 => "Cortex-M0",
        0xC60 => "Cortex-M0+",
        0xC23 => "Cortex-M3",
        0xC24 => "Cortex-M4",
        0xC27 => "Cortex-M7",
        0xD20 => "Cortex-M23",
        0xD21 => "Cortex-M33",
        0xD22 => "Cortex-M55",
        _ => return None,
    })
}

/// Where STM32 chips with this core keep DBGMCU_IDCODE. Only these addresses
/// are read, so no other peripheral's register is touched.
fn idcode_addresses(cpuid: u32) -> &'static [u64] {
    match (cpuid >> 4) & 0xFFF {
        // F0, G0, L0, C0, U0.
        0xC20 | 0xC60 => &[0x4001_5800],
        // F1, F2, F3, F4, L1, L4, G4, WB, WL.
        0xC23 | 0xC24 => &[0xE004_2000],
        // F7, then H7.
        0xC27 => &[0xE004_2000, 0x5C00_1000],
        // U5, L5, WBA, then H5.
        0xD21 => &[0xE004_4000, 0x4402_4000],
        _ => &[],
    }
}

/// The STM32 lines that answer this DEV_ID, as probe-rs's chip names start.
fn stm32_families(dev_id: u16) -> Option<&'static [&'static str]> {
    Some(match dev_id {
        // F0
        0x444 => &["STM32F030", "STM32F031", "STM32F038"],
        0x445 => &["STM32F042", "STM32F048", "STM32F070"],
        0x440 => &["STM32F030", "STM32F051", "STM32F058"],
        0x448 => &["STM32F070", "STM32F071", "STM32F072", "STM32F078"],
        0x442 => &["STM32F030", "STM32F091", "STM32F098"],
        // F1
        0x412 | 0x410 | 0x414 | 0x430 => &["STM32F101", "STM32F102", "STM32F103"],
        0x418 => &["STM32F105", "STM32F107"],
        0x420 | 0x428 => &["STM32F100"],
        // F2
        0x411 => &["STM32F205", "STM32F207", "STM32F215", "STM32F217"],
        // F3
        0x422 => &["STM32F302", "STM32F303", "STM32F358"],
        0x432 => &["STM32F373", "STM32F378"],
        0x438 => &["STM32F303", "STM32F328", "STM32F334"],
        0x439 => &["STM32F301", "STM32F302", "STM32F318"],
        0x446 => &["STM32F302", "STM32F303", "STM32F398"],
        // F4
        0x413 => &["STM32F405", "STM32F407", "STM32F415", "STM32F417"],
        0x419 => &["STM32F427", "STM32F429", "STM32F437", "STM32F439"],
        0x423 | 0x433 => &["STM32F401"],
        0x431 => &["STM32F411"],
        0x421 => &["STM32F446"],
        0x458 => &["STM32F410"],
        0x441 => &["STM32F412"],
        0x463 => &["STM32F413", "STM32F423"],
        0x434 => &["STM32F469", "STM32F479"],
        // F7
        0x449 => &["STM32F745", "STM32F746", "STM32F756"],
        0x451 => &[
            "STM32F765",
            "STM32F767",
            "STM32F769",
            "STM32F777",
            "STM32F779",
        ],
        0x452 => &[
            "STM32F722",
            "STM32F723",
            "STM32F730",
            "STM32F732",
            "STM32F733",
        ],
        // G0
        0x466 => &["STM32G030", "STM32G031", "STM32G041"],
        0x460 => &["STM32G070", "STM32G071", "STM32G081"],
        0x456 => &["STM32G050", "STM32G051", "STM32G061"],
        0x467 => &["STM32G0B0", "STM32G0B1", "STM32G0C1"],
        // G4
        0x468 => &["STM32G431", "STM32G441"],
        0x469 => &[
            "STM32G471",
            "STM32G473",
            "STM32G474",
            "STM32G483",
            "STM32G484",
        ],
        0x479 => &["STM32G491", "STM32G4A1"],
        // H5
        0x474 => &["STM32H503"],
        0x478 => &["STM32H523", "STM32H533"],
        0x484 => &["STM32H562", "STM32H563", "STM32H573"],
        // H7
        0x450 => &[
            "STM32H742",
            "STM32H743",
            "STM32H745",
            "STM32H747",
            "STM32H750",
            "STM32H753",
            "STM32H755",
            "STM32H757",
        ],
        0x480 => &["STM32H7A3", "STM32H7B0", "STM32H7B3"],
        0x483 => &[
            "STM32H723",
            "STM32H725",
            "STM32H730",
            "STM32H733",
            "STM32H735",
        ],
        // L0
        0x457 => &["STM32L010", "STM32L011", "STM32L021"],
        0x425 => &["STM32L031", "STM32L041"],
        0x417 => &[
            "STM32L051",
            "STM32L052",
            "STM32L053",
            "STM32L062",
            "STM32L063",
        ],
        0x447 => &[
            "STM32L071",
            "STM32L072",
            "STM32L073",
            "STM32L082",
            "STM32L083",
        ],
        // L4
        0x415 => &["STM32L475", "STM32L476", "STM32L486"],
        0x461 => &["STM32L496", "STM32L4A6"],
        0x435 => &[
            "STM32L431",
            "STM32L432",
            "STM32L433",
            "STM32L442",
            "STM32L443",
        ],
        0x462 => &["STM32L451", "STM32L452", "STM32L462"],
        0x464 => &["STM32L412", "STM32L422"],
        0x470 => &[
            "STM32L4R5",
            "STM32L4R7",
            "STM32L4R9",
            "STM32L4S5",
            "STM32L4S7",
            "STM32L4S9",
        ],
        0x471 => &["STM32L4P5", "STM32L4Q5"],
        // L5
        0x472 => &["STM32L552", "STM32L562"],
        // U5
        0x482 => &["STM32U575", "STM32U585"],
        0x481 => &["STM32U595", "STM32U599", "STM32U5A5", "STM32U5A9"],
        0x455 => &["STM32U535", "STM32U545"],
        0x476 => &["STM32U5F7", "STM32U5F9", "STM32U5G7", "STM32U5G9"],
        // WB and WL
        0x495 => &["STM32WB50", "STM32WB55", "STM32WB5M"],
        0x494 => &["STM32WB10", "STM32WB15"],
        0x496 => &["STM32WB30", "STM32WB35"],
        0x497 => &["STM32WLE4", "STM32WLE5", "STM32WL54", "STM32WL55"],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cores_come_from_cpuid() {
        // The STM32H503 answered 0x410FD214.
        assert_eq!(core_name(0x410F_D214), Some("Cortex-M33"));
        assert_eq!(core_name(0x410F_C241), Some("Cortex-M4"));
        assert_eq!(core_name(0x410C_C601), Some("Cortex-M0+"));
        // Not ARM's.
        assert_eq!(core_name(0x690F_D214), None);
    }

    #[test]
    fn each_core_reads_only_its_own_dbgmcu() {
        assert_eq!(idcode_addresses(0x410F_D214), [0xE004_4000, 0x4402_4000]);
        assert_eq!(idcode_addresses(0x410C_C601), [0x4001_5800]);
        assert!(idcode_addresses(0x410F_D220).is_empty());
    }

    #[test]
    fn the_h503_is_found_with_its_packages() {
        let found = stm32(0x474, "Cortex-M33");
        assert_eq!(found.chip.as_deref(), Some("STM32H503CB"));
        assert!(
            found.candidates.contains(&"STM32H503RB".to_string()),
            "{found:?}"
        );
        assert!(
            found
                .candidates
                .iter()
                .all(|chip| chip.starts_with("STM32H503"))
        );
        assert_eq!(found.description, "STM32H503 (Cortex-M33, DEV_ID 0x474)");
    }

    #[test]
    fn every_family_in_the_table_is_known_to_probe_rs() {
        // A typo in the table would find no chip at all.
        for dev_id in 0x400..0x500 {
            if let Some(families) = stm32_families(dev_id) {
                for family in families {
                    assert!(
                        !search_chips(family, 1).is_empty(),
                        "probe-rs knows no {family} (DEV_ID {dev_id:#05x})"
                    );
                }
            }
        }
    }

    #[test]
    fn an_unknown_stm32_is_said_so() {
        let found = stm32(0x4FF, "Cortex-M4");
        assert_eq!(found.chip, None);
        assert!(found.description.contains("0x4ff"), "{}", found.description);
    }
}
