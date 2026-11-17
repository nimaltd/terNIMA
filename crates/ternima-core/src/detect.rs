//! Finding out which chip is on the board.
//!
//! probe-rs finds some chips by itself (Microchip SAM, Infineon, Renesas and
//! others) but cannot tell STM32 chips apart, knows only some revisions of
//! the Nordic chips and none of the RP2040. Those are found by reading their
//! ID registers, only reading, so the firmware is not touched. A chip whose
//! firmware keeps the probe out, by sleeping deeply or by taking the debug
//! pins, is read again with its reset line held, which starts the firmware
//! again.

use probe_rs::Permissions;
use probe_rs::architecture::arm::dp::DpAddress;
use probe_rs::architecture::arm::memory::{ArmMemoryInterface, Component};
use probe_rs::architecture::arm::sequences::DefaultArmSequence;
use probe_rs::architecture::arm::{ArmDebugInterface, FullyQualifiedApAddress};
use probe_rs::config::{Registry, TargetSelector};
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
    /// The chip answered only with its reset line held, so a tab on it
    /// connects under reset too.
    pub under_reset: bool,
}

/// Why a look at the chip found nothing.
enum Missed {
    /// The chip did not answer. It may with its reset line held.
    Unreached(String),
    /// It answered, but what it is could not be told.
    Unknown(String),
}

/// Looks at the chip through the probe: without touching it, then, if it does
/// not answer, with its reset line held.
pub fn detect(probe: Option<&ProbeId>, speed_khz: u32) -> Result<Detected, String> {
    match look(probe, speed_khz, false) {
        Ok(found) => Ok(found),
        Err(Missed::Unreached(problem)) => match look(probe, speed_khz, true) {
            Ok(found) => Ok(Detected {
                under_reset: true,
                ..found
            }),
            Err(_) => Err(problem),
        },
        Err(Missed::Unknown(problem)) => Err(problem),
    }
}

/// Reads the chip's IDs, then asks probe-rs about the chips terNIMA does not
/// read itself.
fn look(probe: Option<&ProbeId>, speed_khz: u32, under_reset: bool) -> Result<Detected, Missed> {
    let read = read_ids(probe, speed_khz, under_reset)?;
    if let Some(found) = read.found {
        return Ok(found);
    }
    let opened = probe::open(probe, speed_khz).map_err(Missed::Unreached)?;
    let attached = if under_reset {
        opened.attach_under_reset(TargetSelector::Auto, Permissions::default())
    } else {
        opened.attach(TargetSelector::Auto, Permissions::default())
    };
    match attached {
        Ok(mut session) => {
            if under_reset {
                // probe-rs stops the cores as they leave reset: the firmware runs again.
                for (index, _) in session.list_cores() {
                    if let Ok(mut core) = session.core(index) {
                        let _ = core.run();
                    }
                }
            }
            let name = session.target().name.clone();
            Ok(Detected {
                chip: Some(name.clone()),
                candidates: vec![name.clone()],
                description: name,
                under_reset: false,
            })
        }
        Err(_) => Err(Missed::Unknown(match read.maker {
            Some(maker) => format!(
                "Found a chip by {maker}, but not which one it is. Type its name: probe-rs knows most chips by name."
            ),
            None => {
                "The chip could not be found out. Type its name: probe-rs knows most chips by name."
                    .into()
            }
        })),
    }
}

/// What the chip's ROM table and ID registers tell.
struct Ids {
    /// Who made the chip, as its ROM table says, such as "STMicroelectronics".
    maker: Option<String>,
    /// The chip, when terNIMA tells it by itself: an STM32 or an nRF54L.
    found: Option<Detected>,
}

/// Reads the chip's ROM table and ID registers, with its reset line held if
/// asked, and always lets the line go again.
fn read_ids(probe: Option<&ProbeId>, speed_khz: u32, under_reset: bool) -> Result<Ids, Missed> {
    let mut opened = probe::open(probe, speed_khz).map_err(Missed::Unreached)?;
    let attached = if under_reset {
        opened.attach_to_unspecified_under_reset()
    } else {
        opened.attach_to_unspecified()
    };
    if let Err(error) = attached {
        if under_reset {
            let _ = opened.target_reset_deassert();
        }
        return Err(Missed::Unreached(format!(
            "Could not reach the chip: {error}"
        )));
    }
    let (mut opened, read) = match opened.try_into_arm_debug_interface(DefaultArmSequence::create())
    {
        Ok(mut interface) => {
            let read = read_aps(&mut *interface).or_else(|missed| match missed {
                Missed::Unreached(_) => read_rp2040(&mut *interface).ok_or(missed),
                missed => Err(missed),
            });
            (interface.close(), read)
        }
        Err((opened, error)) => (
            opened,
            Err(Missed::Unreached(format!(
                "Could not reach the chip: {error}"
            ))),
        ),
    };
    if under_reset {
        let _ = opened.target_reset_deassert();
    }
    read
}

/// Reads the access ports the core may sit on: 0 on most chips, and 1 on some,
/// such as the STM32H5.
fn read_aps(interface: &mut dyn ArmDebugInterface) -> Result<Ids, Missed> {
    // The dual-core STM32H7 has its second core on access port 3.
    let second_core = interface
        .memory_interface(&FullyQualifiedApAddress::v1_with_default_dp(3))
        .is_ok();
    let mut reached = false;
    let mut maker = None;
    for ap in [0, 1] {
        let Ok(mut memory) =
            interface.memory_interface(&FullyQualifiedApAddress::v1_with_default_dp(ap))
        else {
            continue;
        };
        let rom = rom_table(&mut *memory);
        if let Some(rom) = &rom {
            reached = true;
            maker = maker.or(rom.maker.clone());
        }
        let Ok(cpuid) = memory.read_word_32(CPUID) else {
            continue;
        };
        reached = true;
        let Some(core) = core_name(cpuid) else {
            continue;
        };
        let found = match &rom {
            Some(rom) if rom.code == NORDIC => {
                nrf54l(&mut *memory, core).or_else(|| nordic(&mut *memory, core))
            }
            Some(rom) => read_stm32(
                &mut *memory,
                cpuid,
                core,
                rom.code == ST,
                rom.part,
                second_core,
            ),
            None => read_stm32(&mut *memory, cpuid, core, false, 0, second_core),
        };
        if found.is_some() {
            return Ok(Ids { maker, found });
        }
    }
    if reached {
        Ok(Ids { maker, found: None })
    } else {
        Err(Missed::Unreached(
            "Could not reach the chip: it does not answer.".into(),
        ))
    }
}

/// What a ROM table says of its chip.
struct RomTable {
    /// The maker's JEP106 code, as (continuation code, id).
    code: (u8, u8),
    maker: Option<String>,
    part: u16,
}

/// STMicroelectronics' and Nordic Semiconductor's JEP106 codes.
const ST: (u8, u8) = (0x00, 0x20);
const NORDIC: (u8, u8) = (0x02, 0x44);

/// The maker and part in the ROM table this access port starts at.
fn rom_table(memory: &mut dyn ArmMemoryInterface) -> Option<RomTable> {
    let base = memory.base_address().ok()?;
    let Ok(Component::Class1RomTable(id, _)) = Component::try_parse(memory, base) else {
        return None;
    };
    let code = id.peripheral_id().jep106()?;
    Some(RomTable {
        code: (code.cc, code.id),
        maker: code.get().map(String::from),
        part: id.peripheral_id().part(),
    })
}

/// An STM32's DEV_ID from DBGMCU_IDCODE, or from its ROM table, which holds
/// the same number, when that cannot be read. Its flash size and package
/// narrow down which part it is, as many parts share a DEV_ID.
fn read_stm32(
    memory: &mut dyn ArmMemoryInterface,
    cpuid: u32,
    core: &str,
    by_st: bool,
    rom_part: u16,
    second_core: bool,
) -> Option<Detected> {
    let dev_id = idcode_addresses(cpuid)
        .iter()
        .filter_map(|&address| memory.read_word_32(address).ok())
        .map(|idcode| (idcode & 0xFFF) as u16)
        .find(|&dev_id| dev_id != 0);
    let dev_id = stm32_dev_id(dev_id, by_st, rom_part)?;
    let flash_kb = stm32_families(dev_id)
        .and_then(|families| flash_size_address(dev_id, families[0]))
        .and_then(|address| read_half(memory, address))
        .map(u32::from);
    let traits = Traits {
        flash_kb,
        package: read_package(memory, dev_id),
        second_core,
    };
    Some(stm32(dev_id, core, &traits))
}

/// The DEV_ID of an STM32 or of a clone of one: a chip by another maker counts
/// only when its DEV_ID is an STM32's, so its other registers are not taken
/// for one.
fn stm32_dev_id(dev_id: Option<u16>, by_st: bool, rom_part: u16) -> Option<u16> {
    let dev_id = dev_id
        .or_else(|| by_st.then_some(rom_part & 0xFFF))
        .filter(|&dev_id| dev_id != 0)?;
    (by_st || stm32_families(dev_id).is_some()).then_some(dev_id)
}

/// What an STM32 tells of itself besides its DEV_ID.
#[derive(Clone, Copy, Debug, Default)]
struct Traits {
    /// Its flash, in KB.
    flash_kb: Option<u32>,
    /// Its package register, as ST numbers the packages.
    package: Option<u32>,
    /// Whether a second core answers on access port 3, as the dual-core STM32H7's.
    second_core: bool,
}

/// What an STM32's DEV_ID says, with the chips probe-rs knows by those names,
/// narrowed down by its flash size, package and cores.
fn stm32(dev_id: u16, core: &str, traits: &Traits) -> Detected {
    let Some(families) = stm32_families(dev_id) else {
        return Detected {
            chip: None,
            candidates: Vec::new(),
            description: format!(
                "an STM32 with DEV_ID {dev_id:#05x} ({core}) that terNIMA does not know yet"
            ),
            under_reset: false,
        };
    };
    let mut candidates: Vec<String> = families
        .iter()
        .flat_map(|family| search_chips(family, 100))
        .filter(|chip| families.iter().any(|family| chip.starts_with(family)))
        .collect();
    let mut known = vec![format!("{core}, DEV_ID {dev_id:#05x}")];

    // The lines with and without a second core share the STM32H7's DEV_ID.
    if dev_id == 0x450 {
        narrow(&mut candidates, |chip| {
            DUAL_CORE.iter().any(|line| chip.starts_with(line)) == traits.second_core
        });
    }
    // The letters after the line, as in STM32H743ZI: Z is the package, I the flash size.
    let letters_of = |chip: &str| part_letters(chip, families);
    if let Some(kb) = traits.flash_kb
        && let Some(letter) = flash_letter(kb)
        && narrow(&mut candidates, |chip| {
            letters_of(chip).is_some_and(|(_, flash)| flash == letter)
        })
    {
        known.push(format!("{} flash", size_text(kb)));
    }
    if let Some((name, letters)) = traits
        .package
        .and_then(|code| package(dev_id, code, traits.second_core))
        && narrow(&mut candidates, |chip| {
            letters_of(chip).is_some_and(|(package, _)| letters.contains(package))
        })
    {
        known.push(name.into());
    }

    let lines: Vec<&str> = families
        .iter()
        .copied()
        .filter(|family| candidates.iter().any(|chip| chip.starts_with(family)))
        .collect();
    Detected {
        chip: candidates.first().cloned(),
        description: format!("{} ({})", lines.join(", "), known.join(", ")),
        candidates,
        under_reset: false,
    }
}

/// A part's package and flash letters: the two after its line, as Z and I in
/// STM32H743ZI, or C and G in STM32WBA55CG.
fn part_letters(chip: &str, families: &[&str]) -> Option<(char, char)> {
    let line = families
        .iter()
        .filter(|family| chip.starts_with(*family))
        .max_by_key(|family| family.len())?;
    let mut rest = chip[line.len()..].chars();
    Some((rest.next()?, rest.next()?))
}

/// Keeps the chips that match, unless none does: what was read may be wrong.
/// Gives whether it narrowed the list.
fn narrow(candidates: &mut Vec<String>, matches: impl Fn(&str) -> bool) -> bool {
    if !candidates.iter().any(|chip| matches(chip)) {
        return false;
    }
    candidates.retain(|chip| matches(chip));
    true
}

/// The STM32H7 lines with a second core, a Cortex-M4.
const DUAL_CORE: [&str; 4] = ["STM32H745", "STM32H747", "STM32H755", "STM32H757"];

/// Where an STM32 keeps its flash size in KB, as FLASHSIZE_BASE in ST's headers.
/// None where it is elsewhere or not known: the list is not narrowed then.
fn flash_size_address(dev_id: u16, family: &str) -> Option<u64> {
    // The STM32L1 keeps it in one of two places, by category.
    match dev_id {
        0x416 | 0x429 => return Some(0x1FF8_004C),
        0x427 | 0x436 | 0x437 => return Some(0x1FF8_00CC),
        _ => {}
    }
    const PLACES: [(&str, Option<u64>); 24] = [
        ("STM32C0", Some(0x1FFF_75A0)),
        ("STM32F0", Some(0x1FFF_F7CC)),
        ("STM32F3", Some(0x1FFF_F7CC)),
        ("STM32F2", Some(0x1FFF_7A22)),
        ("STM32F4", Some(0x1FFF_7A22)),
        ("STM32F72", Some(0x1FF0_7A22)),
        ("STM32F73", Some(0x1FF0_7A22)),
        ("STM32F7", Some(0x1FF0_F442)),
        ("STM32G0", Some(0x1FFF_75E0)),
        ("STM32G4", Some(0x1FFF_75E0)),
        ("STM32L4", Some(0x1FFF_75E0)),
        ("STM32WBA", None),
        ("STM32WB", Some(0x1FFF_75E0)),
        ("STM32WL", Some(0x1FFF_75E0)),
        ("STM32H7R", None),
        ("STM32H7S", None),
        ("STM32H7A", Some(0x08FF_F80C)),
        ("STM32H7B", Some(0x08FF_F80C)),
        ("STM32H7", Some(0x1FF1_E880)),
        ("STM32H5", Some(0x08FF_F80C)),
        ("STM32L0", Some(0x1FF8_007C)),
        ("STM32L5", Some(0x0BFA_05E0)),
        ("STM32U0", Some(0x1FFF_3EA0)),
        ("STM32U5", Some(0x0BFA_07A0)),
    ];
    PLACES
        .iter()
        .find(|(prefix, _)| family.starts_with(prefix))
        .and_then(|&(_, address)| address)
}

/// A 16-bit value, read through the 32-bit word it sits in.
fn read_half(memory: &mut dyn ArmMemoryInterface, address: u64) -> Option<u16> {
    let word = memory.read_word_32(address & !3).ok()?;
    Some((word >> ((address & 3) * 8)) as u16)
}

/// The letter for a flash size in an STM32's name.
fn flash_letter(kb: u32) -> Option<char> {
    Some(match kb {
        16 => '4',
        32 => '6',
        64 => '8',
        128 => 'B',
        192 => 'Z',
        256 => 'C',
        384 => 'D',
        512 => 'E',
        768 => 'F',
        1024 => 'G',
        1536 => 'H',
        2048 => 'I',
        4096 => 'J',
        _ => return None,
    })
}

fn size_text(kb: u32) -> String {
    if kb >= 1024 && kb.is_multiple_of(1024) {
        format!("{} MB", kb / 1024)
    } else {
        format!("{kb} KB")
    }
}

/// The STM32H7's SYSCFG_PKGR, and RCC_APB4ENR with SYSCFG's clock bit.
const SYSCFG_PKGR: u64 = 0x5800_0524;
const RCC_APB4ENR: u64 = 0x5802_44F4;
const SYSCFGEN: u32 = 1 << 1;
/// The STM32G4's package data, in system memory.
const G4_PACKAGE: u64 = 0x1FFF_7500;

/// The package register of the STM32s whose package codes are known. The
/// STM32H7's is read only while the firmware has SYSCFG's clock on, as it
/// reads 0 otherwise.
fn read_package(memory: &mut dyn ArmMemoryInterface, dev_id: u16) -> Option<u32> {
    match dev_id {
        0x450 | 0x483 => {
            let clocks = memory.read_word_32(RCC_APB4ENR).ok()?;
            if clocks & SYSCFGEN == 0 {
                return None;
            }
            Some(memory.read_word_32(SYSCFG_PKGR).ok()? & 0xF)
        }
        0x468 | 0x469 | 0x479 => Some(memory.read_word_32(G4_PACKAGE).ok()? & 0x1F),
        _ => None,
    }
}

/// A package by its code, as ST's LL headers number them, with the letters it
/// has in a part's name: V for 100 pins, Z for 144 and so on.
fn package(dev_id: u16, code: u32, second_core: bool) -> Option<(&'static str, &'static str)> {
    Some(match (dev_id, second_core, code) {
        // STM32H742, H743, H750 and H753.
        (0x450, false, 0) => ("LQFP100", "V"),
        (0x450, false, 2) => ("TQFP144", "Z"),
        (0x450, false, 5) => ("TQFP176 or UFBGA176", "I"),
        (0x450, false, 8) => ("LQFP208 or TFBGA240", "BX"),
        // STM32H745, H747, H755 and H757.
        (0x450, true, 2) => ("UFBGA169 or LQFP176", "AI"),
        (0x450, true, 3) => ("LQFP144", "Z"),
        (0x450, true, 6) => ("LQFP176", "I"),
        (0x450, true, 7) => ("UFBGA176", "I"),
        (0x450, true, 9 | 10) => ("LQFP208", "B"),
        // STM32H72x and H73x.
        (0x483, _, 1) => ("TFBGA100 or LQFP100", "V"),
        (0x483, _, 2) => ("LQFP100", "V"),
        (0x483, _, 3) => ("TFBGA100", "V"),
        (0x483, _, 5 | 7) => ("LQFP144", "Z"),
        (0x483, _, 6) => ("UFBGA144", "Z"),
        (0x483, _, 8) => ("UFBGA169", "A"),
        (0x483, _, 9) => ("UFBGA176+25", "I"),
        (0x483, _, 10) => ("LQFP176", "I"),
        // STM32G4.
        (0x468 | 0x469 | 0x479, _, 0) => ("LQFP64", "R"),
        (0x468 | 0x469 | 0x479, _, 1) => ("WLCSP64", "R"),
        (0x468 | 0x469 | 0x479, _, 2) => ("LQFP100 or LQFP80", "VM"),
        (0x468 | 0x469 | 0x479, _, 5) => ("WLCSP81", "M"),
        (0x468 | 0x469 | 0x479, _, 7) => ("LQFP128 or UFBGA121", "QP"),
        (0x468 | 0x469 | 0x479, _, 8) => ("UFQFPN32", "K"),
        (0x468 | 0x469 | 0x479, _, 9) => ("LQFP32", "K"),
        (0x468 | 0x469 | 0x479, _, 10) => ("UFQFPN48", "C"),
        (0x468 | 0x469 | 0x479, _, 11 | 16) => ("LQFP48", "C"),
        (0x468 | 0x469 | 0x479, _, 12) => ("WLCSP49", "C"),
        (0x468 | 0x469 | 0x479, _, 13) => ("UFBGA64", "R"),
        (0x468 | 0x469 | 0x479, _, 14) => ("TFBGA100", "V"),
        (0x468 | 0x469 | 0x479, _, 17) => ("LQFP80", "M"),
        _ => return None,
    })
}

/// Where an nRF54L keeps its part and variant: FICR INFO.PART and INFO.VARIANT.
const NRF54L_PART: u64 = 0x00FF_C31C;
const NRF54L_VARIANT: u64 = 0x00FF_C320;

/// An nRF54L, from its FICR.
fn nrf54l(memory: &mut dyn ArmMemoryInterface, core: &str) -> Option<Detected> {
    let part = memory.read_word_32(NRF54L_PART).ok()?;
    let variant = memory.read_word_32(NRF54L_VARIANT).ok();
    nrf54l_named(part, variant, core)
}

/// An nRF54L named by its part alone: probe-rs's own table has only one early
/// variant of the nRF54L15, so other revisions are not found by it.
fn nrf54l_named(part: u32, variant: Option<u32>, core: &str) -> Option<Detected> {
    let name = match part {
        0x54B15 => "nRF54L15",
        0x54B10 => "nRF54L10",
        0x54B05 => "nRF54L05",
        _ => return None,
    };
    let candidates: Vec<String> = search_chips(name, 10)
        .into_iter()
        .filter(|chip| chip.starts_with(name))
        .collect();
    let variant = variant
        .and_then(variant_text)
        .map(|text| format!(", variant {text}"))
        .unwrap_or_default();
    let known = if candidates.is_empty() {
        ", which probe-rs does not know yet"
    } else {
        ""
    };
    Some(Detected {
        chip: candidates.first().cloned(),
        candidates,
        description: format!("{name} ({core}{variant}){known}"),
        under_reset: false,
    })
}

/// A Nordic chip from its FICR, at the places and with the parts in
/// probe-rs's own tables. Those list only some revisions of each variant, so
/// a chip is named by its variant's first two letters, which tell its memory,
/// as AA in nRF52840_xxAA: a newer revision is found too.
fn nordic(memory: &mut dyn ArmMemoryInterface, core: &str) -> Option<Detected> {
    let registry = Registry::from_builtin_families();
    for family in registry.families() {
        for ficr in family
            .chip_detection
            .iter()
            .filter_map(|method| method.as_nordic_ficr())
        {
            let Ok(part) = memory.read_word_32(u64::from(ficr.part_address)) else {
                continue;
            };
            if part != ficr.part {
                continue;
            }
            let variant = memory.read_word_32(u64::from(ficr.variant_address)).ok();
            let names = ficr
                .variants
                .iter()
                .map(|(&code, name)| (code, name.as_str()));
            if let Some(found) = nordic_named(variant, names, core) {
                return Some(found);
            }
        }
    }
    None
}

/// The chip a Nordic variant is, among a part's variants and their names:
/// the variant itself, or else one with the same memory letters, or else the
/// first. The others stay in the list to pick from.
fn nordic_named<'a>(
    variant: Option<u32>,
    names: impl Iterator<Item = (u32, &'a str)>,
    core: &str,
) -> Option<Detected> {
    let names: Vec<(u32, &str)> = names.collect();
    let memory_letters = |code: u32| code >> 16;
    let chosen = variant
        .and_then(|variant| {
            names.iter().find(|(code, _)| *code == variant).or_else(|| {
                names
                    .iter()
                    .find(|(code, _)| memory_letters(*code) == memory_letters(variant))
            })
        })
        .or(names.first())?
        .1;
    let mut candidates = vec![chosen.to_string()];
    for (_, name) in &names {
        if !candidates.iter().any(|known| known == name) {
            candidates.push(name.to_string());
        }
    }
    let variant = variant
        .and_then(variant_text)
        .map(|text| format!(", variant {text}"))
        .unwrap_or_default();
    Some(Detected {
        chip: Some(chosen.to_string()),
        candidates,
        description: format!("{chosen} ({core}{variant})"),
        under_reset: false,
    })
}

/// The RP2040's two cores answer only on a multi-drop SWD bus, each at an
/// address of its own: core 0's here. A probe that cannot reach such a bus,
/// as an ST-Link, finds nothing.
const RP2040_CORE0: u32 = 0x0100_2927;

/// An RP2040, when its core 0 answers at its own address on the bus.
fn read_rp2040(interface: &mut dyn ArmDebugInterface) -> Option<Ids> {
    let core0 = FullyQualifiedApAddress::v1_with_dp(DpAddress::Multidrop(RP2040_CORE0), 0);
    let mut memory = interface.memory_interface(&core0).ok()?;
    let core = core_name(memory.read_word_32(CPUID).ok()?)?;
    Some(Ids {
        maker: Some("Raspberry Pi".into()),
        found: Some(Detected {
            chip: Some("RP2040".into()),
            candidates: vec!["RP2040".into()],
            description: format!("RP2040 ({core})"),
            under_reset: false,
        }),
    })
}

/// A Nordic variant in its letters, such as "AABB" for 0x41414242.
fn variant_text(variant: u32) -> Option<String> {
    let bytes = variant.to_be_bytes();
    bytes
        .iter()
        .all(u8::is_ascii_alphanumeric)
        .then(|| bytes.iter().map(|&byte| char::from(byte)).collect())
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
        // C0
        0x443 => &["STM32C011"],
        0x453 => &["STM32C031"],
        0x44C => &["STM32C051"],
        0x493 => &["STM32C071"],
        0x44D => &["STM32C091", "STM32C092"],
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
        0x449 => &["STM32F745", "STM32F746", "STM32F750", "STM32F756"],
        0x451 => &[
            "STM32F765",
            "STM32F767",
            "STM32F768",
            "STM32F769",
            "STM32F777",
            "STM32F778",
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
        // The H743 first: the most common of them, as on the NUCLEO-H743ZI.
        0x450 => &[
            "STM32H743",
            "STM32H742",
            "STM32H753",
            "STM32H750",
            "STM32H745",
            "STM32H747",
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
        0x485 => &["STM32H7R3", "STM32H7R7", "STM32H7S3", "STM32H7S7"],
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
            "STM32L081",
            "STM32L082",
            "STM32L083",
        ],
        // L1: categories 1 and 2, 3, then 4 to 6.
        0x416 | 0x429 => &["STM32L100", "STM32L151", "STM32L152"],
        0x427 => &["STM32L100", "STM32L151", "STM32L152", "STM32L162"],
        0x436 | 0x437 => &["STM32L151", "STM32L152", "STM32L162"],
        // L4
        0x415 => &["STM32L471", "STM32L475", "STM32L476", "STM32L486"],
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
        // U0
        0x459 => &["STM32U031"],
        0x489 => &["STM32U073", "STM32U083"],
        // U3
        0x454 => &["STM32U375", "STM32U385"],
        // U5
        0x482 => &["STM32U575", "STM32U585"],
        0x481 => &["STM32U595", "STM32U599", "STM32U5A5", "STM32U5A9"],
        0x455 => &["STM32U535", "STM32U545"],
        0x476 => &["STM32U5F7", "STM32U5F9", "STM32U5G7", "STM32U5G9"],
        // WB and WL
        0x495 => &["STM32WB50", "STM32WB55", "STM32WB5M"],
        0x494 => &["STM32WB10", "STM32WB15", "STM32WB1M"],
        0x496 => &["STM32WB30", "STM32WB35"],
        0x497 => &[
            "STM32WLE4",
            "STM32WLE5",
            "STM32WL54",
            "STM32WL55",
            "STM32WL5M",
        ],
        // WBA
        0x492 => &[
            "STM32WBA50",
            "STM32WBA52",
            "STM32WBA54",
            "STM32WBA55",
            "STM32WBA5M",
        ],
        0x4B0 => &["STM32WBA62", "STM32WBA63", "STM32WBA64", "STM32WBA65"],
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
        let found = stm32(0x474, "Cortex-M33", &Traits::default());
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
        let found = stm32(0x4FF, "Cortex-M4", &Traits::default());
        assert_eq!(found.chip, None);
        assert!(found.description.contains("0x4ff"), "{}", found.description);
    }

    #[test]
    fn only_st_chips_and_their_clones_count_as_stm32() {
        // The G431 answered DEV_ID 0x468, in DBGMCU_IDCODE and in its ROM table.
        assert_eq!(stm32_dev_id(Some(0x468), true, 0x468), Some(0x468));
        // From the ROM table, when DBGMCU_IDCODE cannot be read.
        assert_eq!(stm32_dev_id(None, true, 0x468), Some(0x468));
        // A clone by another maker answers an STM32's DEV_ID.
        assert_eq!(stm32_dev_id(Some(0x413), false, 0), Some(0x413));
        // Another maker's register is not taken for a DEV_ID, nor its part number.
        assert_eq!(stm32_dev_id(Some(0x123), false, 0), None);
        assert_eq!(stm32_dev_id(None, false, 0x468), None);
        // An ST chip terNIMA does not know is still said to be an STM32.
        assert_eq!(stm32_dev_id(Some(0x4FF), true, 0), Some(0x4FF));
    }

    #[test]
    fn the_h743_is_told_by_its_flash_package_and_one_core() {
        // What the NUCLEO-H743ZI answered: 2 MB, TQFP144, nothing on access port 3.
        let traits = Traits {
            flash_kb: Some(2048),
            package: Some(2),
            second_core: false,
        };
        let found = stm32(0x450, "Cortex-M7", &traits);
        assert_eq!(found.chip.as_deref(), Some("STM32H743ZI"));
        assert_eq!(
            found.candidates,
            ["STM32H743ZI", "STM32H742ZI", "STM32H753ZI"]
        );
        assert_eq!(
            found.description,
            "STM32H743, STM32H742, STM32H753 (Cortex-M7, DEV_ID 0x450, 2 MB flash, TQFP144)"
        );
    }

    #[test]
    fn a_second_core_means_a_dual_core_h7() {
        let traits = Traits {
            flash_kb: Some(2048),
            package: Some(3),
            second_core: true,
        };
        let found = stm32(0x450, "Cortex-M7", &traits);
        assert!(
            found
                .candidates
                .iter()
                .all(|chip| DUAL_CORE.iter().any(|line| chip.starts_with(line))
                    && chip.ends_with("ZI")),
            "{:?}",
            found.candidates
        );
        assert!(!found.candidates.is_empty());
    }

    #[test]
    fn the_g431_is_told_by_its_flash_and_package() {
        // The NUCLEO-G431RB: 128 KB, LQFP64.
        let traits = Traits {
            flash_kb: Some(128),
            package: Some(0),
            second_core: false,
        };
        let found = stm32(0x468, "Cortex-M4", &traits);
        assert_eq!(found.chip.as_deref(), Some("STM32G431RB"));
        assert_eq!(found.candidates, ["STM32G431RB", "STM32G441RB"]);
    }

    #[test]
    fn what_matches_no_part_is_left_out() {
        // A flash size or package no part has keeps the whole list.
        let traits = Traits {
            flash_kb: Some(4096),
            package: Some(31),
            second_core: false,
        };
        let found = stm32(0x468, "Cortex-M4", &traits);
        assert_eq!(found.chip.as_deref(), Some("STM32G431C6"));
        assert_eq!(found.candidates.len(), 20);
        assert_eq!(
            found.description,
            "STM32G431, STM32G441 (Cortex-M4, DEV_ID 0x468)"
        );
    }

    #[test]
    fn flash_sizes_read_as_their_letters() {
        assert_eq!(flash_letter(2048), Some('I'));
        assert_eq!(flash_letter(128), Some('B'));
        assert_eq!(flash_letter(100), None);
        assert_eq!(size_text(2048), "2 MB");
        assert_eq!(size_text(128), "128 KB");
        assert_eq!(flash_size_address(0x450, "STM32H743"), Some(0x1FF1_E880));
        assert_eq!(flash_size_address(0x480, "STM32H7A3"), Some(0x08FF_F80C));
        assert_eq!(flash_size_address(0x452, "STM32F722"), Some(0x1FF0_7A22));
        assert_eq!(flash_size_address(0x449, "STM32F746"), Some(0x1FF0_F442));
        // The STM32L1 by its category, and the lines whose place is not known.
        assert_eq!(flash_size_address(0x416, "STM32L100"), Some(0x1FF8_004C));
        assert_eq!(flash_size_address(0x437, "STM32L151"), Some(0x1FF8_00CC));
        assert_eq!(flash_size_address(0x492, "STM32WBA50"), None);
        assert_eq!(flash_size_address(0x485, "STM32H7R3"), None);
        assert_eq!(flash_size_address(0x495, "STM32WB50"), Some(0x1FFF_75E0));
    }

    #[test]
    fn the_letters_follow_the_line_however_long_it_is() {
        let families = &["STM32WBA55", "STM32H743"][..];
        assert_eq!(part_letters("STM32WBA55CG", families), Some(('C', 'G')));
        assert_eq!(part_letters("STM32H743ZI", families), Some(('Z', 'I')));
        assert_eq!(part_letters("STM32F103C8", families), None);
    }

    #[test]
    fn the_newer_lines_are_named() {
        for (dev_id, line) in [
            (0x443, "STM32C011"),
            (0x453, "STM32C031"),
            (0x416, "STM32L151"),
            (0x437, "STM32L162"),
            (0x459, "STM32U031"),
            (0x489, "STM32U083"),
            (0x492, "STM32WBA55"),
            (0x485, "STM32H7S3"),
        ] {
            let found = stm32(dev_id, "Cortex-M", &Traits::default());
            assert!(
                found.candidates.iter().any(|chip| chip.starts_with(line)),
                "{dev_id:#05x}: {:?}",
                found.candidates
            );
        }
        // The flash letter of an STM32WBA sits one place later than on most.
        let traits = Traits {
            flash_kb: Some(1024),
            ..Traits::default()
        };
        let found = stm32(0x492, "Cortex-M33", &traits);
        assert!(
            found.candidates.iter().all(|chip| chip.ends_with('G')),
            "{:?}",
            found.candidates
        );
    }

    #[test]
    fn an_nrf54l15_is_found_whatever_its_variant() {
        // probe-rs's own table has only 0x41414242, an engineering sample.
        for variant in [0x4141_4242, 0x4141_4330, 0x4141_4130] {
            let found = nrf54l_named(0x54B15, Some(variant), "Cortex-M33").unwrap();
            assert_eq!(found.chip.as_deref(), Some("nRF54L15"), "{variant:#x}");
        }
        let found = nrf54l_named(0x54B15, Some(0x4141_4242), "Cortex-M33").unwrap();
        assert_eq!(found.description, "nRF54L15 (Cortex-M33, variant AABB)");
    }

    #[test]
    fn an_nrf54l_probe_rs_does_not_know_is_said_so() {
        let found = nrf54l_named(0x54B10, None, "Cortex-M33").unwrap();
        assert_eq!(found.chip, None);
        assert_eq!(
            found.description,
            "nRF54L10 (Cortex-M33), which probe-rs does not know yet"
        );
    }

    #[test]
    fn other_nordic_chips_are_left_to_probe_rs() {
        assert_eq!(nrf54l_named(0x52840, None, "Cortex-M4"), None);
        assert_eq!(nrf54l_named(0xFFFF_FFFF, None, "Cortex-M4"), None);
    }

    #[test]
    fn a_nordic_chip_of_a_newer_revision_is_found_by_its_memory_letters() {
        // AAF0: a revision probe-rs's table does not list.
        let names = [(0x4141_4430, "nRF52840_xxAA")];
        let found = nordic_named(Some(0x4141_4630), names.into_iter(), "Cortex-M4").unwrap();
        assert_eq!(found.chip.as_deref(), Some("nRF52840_xxAA"));
        assert_eq!(found.description, "nRF52840_xxAA (Cortex-M4, variant AAF0)");
        // The nRF52832 comes with 512 KB (AA) and with 256 KB (AB).
        let names = [
            (0x4141_4230, "nRF52832_xxAA"),
            (0x4142_4230, "nRF52832_xxAB"),
        ];
        let found = nordic_named(Some(0x4142_4530), names.into_iter(), "Cortex-M4").unwrap();
        assert_eq!(found.chip.as_deref(), Some("nRF52832_xxAB"));
        assert_eq!(found.candidates, ["nRF52832_xxAB", "nRF52832_xxAA"]);
        // A variant read wrong still names the part.
        let found = nordic_named(None, names.into_iter(), "Cortex-M4").unwrap();
        assert_eq!(found.chip.as_deref(), Some("nRF52832_xxAA"));
    }

    #[test]
    fn every_nordic_part_probe_rs_knows_is_named_in_any_revision() {
        let registry = Registry::from_builtin_families();
        let mut parts = 0;
        for family in registry.families() {
            for ficr in family
                .chip_detection
                .iter()
                .filter_map(|method| method.as_nordic_ficr())
            {
                parts += 1;
                for &code in ficr.variants.keys() {
                    // The same memory letters, with a revision no table lists.
                    let newer = (code & 0xFFFF_0000) | 0x5A5A;
                    let names = ficr
                        .variants
                        .iter()
                        .map(|(&code, name)| (code, name.as_str()));
                    let found = nordic_named(Some(newer), names, "Cortex-M4").unwrap();
                    let chip = found.chip.unwrap();
                    assert!(
                        ficr.variants
                            .iter()
                            .any(|(&other, name)| other >> 16 == code >> 16 && *name == chip),
                        "{code:#x} named {chip}"
                    );
                }
            }
        }
        assert!(parts >= 5, "only {parts} Nordic parts");
    }

    #[test]
    fn the_rp2040_is_known_to_probe_rs_by_that_name() {
        assert!(search_chips("RP2040", 5).contains(&"RP2040".to_string()));
    }

    #[test]
    fn variants_read_as_letters() {
        assert_eq!(variant_text(0x4141_4242).as_deref(), Some("AABB"));
        assert_eq!(variant_text(0x4141_4330).as_deref(), Some("AAC0"));
        assert_eq!(variant_text(0xFFFF_FFFF), None);
    }
}
