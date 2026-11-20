//! What the RTOS walkers need from an ELF file: where the kernel keeps its
//! lists, from the symbol table, and how its structs are laid out, from the
//! debug information.

use std::collections::{HashMap, HashSet};

use gimli::{AttributeValue, UnitOffset};
use object::{Object, ObjectSymbol, SymbolKind};

use crate::elf::{
    Dwarf, Unit, array_length, fixed_address, member_offset, name_of, size_of, type_of, with_dwarf,
};

/// How deep structs in structs are opened.
const DEPTH_MAX: usize = 8;
/// The most elements of an array of a wanted struct taken as instances.
const INSTANCES_MAX: u64 = 16;

/// A member of a struct: where it is in it, and how many bytes it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub offset: u64,
    pub size: u64,
}

/// A struct's size and its members, also those of the structs and unions in
/// it, by path: `base.prio`, `stack_info.start`, `cpus[0].current`. An
/// anonymous union lends its members to the struct around it, as in C.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Struct {
    pub size: u64,
    pub fields: HashMap<String, Field>,
}

impl Struct {
    pub fn field(&self, path: &str) -> Option<Field> {
        self.fields.get(path).copied()
    }
}

/// What an ELF file tells of its kernel.
#[derive(Debug, Default)]
pub struct Debug {
    /// Data by name, with its address and size. A static the linker renamed,
    /// as `pxReadyTasksLists.lto_priv.0`, is also under its own name.
    pub data: HashMap<String, (u64, u64)>,
    /// Functions by address, without the Thumb bit.
    pub functions: HashMap<u64, String>,
    /// The structs asked for, by their name or the name of a typedef of them.
    pub structs: HashMap<String, Struct>,
    /// The variables of the structs asked for, as `z_main_thread` or
    /// `z_idle_threads[0]`, with their addresses, by the struct's name.
    pub instances: HashMap<String, Vec<(String, u64)>>,
}

/// Reads the symbols, these structs' layouts, and the variables of the
/// `instances` structs.
pub fn read(data: &[u8], structs: &[&str], instances: &[&str]) -> Result<Debug, String> {
    let file =
        object::File::parse(data).map_err(|error| format!("It is not an ELF file: {error}"))?;
    let mut debug = Debug::default();
    for symbol in file.symbols() {
        let Ok(name) = symbol.name() else {
            continue;
        };
        if name.is_empty() || !symbol.is_definition() {
            continue;
        }
        match symbol.kind() {
            SymbolKind::Data => {
                let place = (symbol.address(), symbol.size());
                debug.data.entry(name.to_string()).or_insert(place);
                if let Some((own, _)) = name.split_once('.') {
                    debug.data.entry(own.to_string()).or_insert(place);
                }
            }
            SymbolKind::Text => {
                debug
                    .functions
                    .entry(symbol.address() & !1)
                    .or_insert_with(|| name.to_string());
            }
            _ => {}
        }
    }
    let structs: HashSet<&str> = structs.iter().copied().collect();
    let instances: HashSet<&str> = instances.iter().copied().collect();
    with_dwarf(&file, |dwarf| scan(dwarf, &structs, &instances, &mut debug))
        .map_err(|error| format!("Its debug information could not be read: {error}"))?;
    Ok(debug)
}

fn scan(
    dwarf: &Dwarf,
    structs: &HashSet<&str>,
    instances: &HashSet<&str>,
    debug: &mut Debug,
) -> gimli::Result<()> {
    let mut seen = HashSet::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            match entry.tag() {
                gimli::DW_TAG_structure_type | gimli::DW_TAG_typedef => {
                    let Some(name) = name_of(dwarf, &unit, entry)? else {
                        continue;
                    };
                    if !structs.contains(name.as_str()) || debug.structs.contains_key(&name) {
                        continue;
                    }
                    if let Some(offset) = defined_struct(&unit, entry.offset())? {
                        let mut layout = Struct {
                            size: size_of(&unit, offset, 0)?.unwrap_or(0),
                            fields: HashMap::new(),
                        };
                        flatten(dwarf, &unit, offset, "", 0, 0, &mut layout.fields)?;
                        debug.structs.insert(name, layout);
                    }
                }
                gimli::DW_TAG_variable if !instances.is_empty() => {
                    let Some(address) = fixed_address(dwarf, &unit, entry)? else {
                        continue;
                    };
                    // A definition after an extern declaration has its name and type there.
                    let declaration = match entry.attr_value(gimli::DW_AT_specification) {
                        Some(AttributeValue::UnitRef(offset)) => Some(unit.entry(offset)?),
                        _ => None,
                    };
                    let name = match name_of(dwarf, &unit, entry)? {
                        Some(name) => Some(name),
                        None => match &declaration {
                            Some(declaration) => name_of(dwarf, &unit, declaration)?,
                            None => None,
                        },
                    };
                    let of = type_of(entry).or_else(|| declaration.as_ref().and_then(type_of));
                    let (Some(name), Some(of)) = (name, of) else {
                        continue;
                    };
                    if !seen.insert((name.clone(), address)) {
                        continue;
                    }
                    instances_of(dwarf, &unit, of, &name, address, instances, debug)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// The struct a type is, past its typedefs, const and volatile: one with its
/// members, not a declaration.
fn defined_struct(unit: &Unit, offset: UnitOffset) -> gimli::Result<Option<UnitOffset>> {
    let offset = strip(unit, offset)?;
    let entry = unit.entry(offset)?;
    let defined = matches!(
        entry.tag(),
        gimli::DW_TAG_structure_type | gimli::DW_TAG_union_type
    ) && !entry.has_attr(gimli::DW_AT_declaration);
    Ok(defined.then_some(offset))
}

/// The type past its typedefs, const and volatile.
fn strip(unit: &Unit, mut offset: UnitOffset) -> gimli::Result<UnitOffset> {
    for _ in 0..DEPTH_MAX * 2 {
        let entry = unit.entry(offset)?;
        match entry.tag() {
            gimli::DW_TAG_typedef
            | gimli::DW_TAG_const_type
            | gimli::DW_TAG_volatile_type
            | gimli::DW_TAG_atomic_type => match type_of(&entry) {
                Some(next) => offset = next,
                None => break,
            },
            _ => break,
        }
    }
    Ok(offset)
}

/// The name a type is known by, past const and volatile: a typedef's or a struct's.
fn type_names(dwarf: &Dwarf, unit: &Unit, mut offset: UnitOffset) -> gimli::Result<Vec<String>> {
    let mut names = Vec::new();
    for _ in 0..DEPTH_MAX * 2 {
        let entry = unit.entry(offset)?;
        if let Some(name) = name_of(dwarf, unit, &entry)? {
            names.push(name);
        }
        match entry.tag() {
            gimli::DW_TAG_typedef
            | gimli::DW_TAG_const_type
            | gimli::DW_TAG_volatile_type
            | gimli::DW_TAG_atomic_type => match type_of(&entry) {
                Some(next) => offset = next,
                None => break,
            },
            _ => break,
        }
    }
    Ok(names)
}

/// A variable of a wanted struct, or an array of them, taken as instances.
fn instances_of(
    dwarf: &Dwarf,
    unit: &Unit,
    of: UnitOffset,
    name: &str,
    address: u64,
    wanted: &HashSet<&str>,
    debug: &mut Debug,
) -> gimli::Result<()> {
    if let Some(found) = type_names(dwarf, unit, of)?
        .into_iter()
        .find(|type_name| wanted.contains(type_name.as_str()))
    {
        let entry = unit.entry(strip(unit, of)?)?;
        if entry.tag() == gimli::DW_TAG_structure_type {
            debug
                .instances
                .entry(found)
                .or_default()
                .push((name.to_string(), address));
        }
        return Ok(());
    }
    let array = strip(unit, of)?;
    let entry = unit.entry(array)?;
    if entry.tag() != gimli::DW_TAG_array_type {
        return Ok(());
    }
    let (Some(element), Some(count)) = (type_of(&entry), array_length(unit, array)?) else {
        return Ok(());
    };
    let Some(found) = type_names(dwarf, unit, element)?
        .into_iter()
        .find(|type_name| wanted.contains(type_name.as_str()))
    else {
        return Ok(());
    };
    let Some(stride) = size_of(unit, element, 0)? else {
        return Ok(());
    };
    let list = debug.instances.entry(found).or_default();
    for index in 0..count.min(INSTANCES_MAX) {
        list.push((format!("{name}[{index}]"), address + index * stride));
    }
    Ok(())
}

/// Adds a struct's members, and theirs, under `prefix`, `base` bytes into
/// the struct asked for. An array of structs gives its first element's, as
/// `cpus[0].current`, and its stride as `cpus[]`.
fn flatten(
    dwarf: &Dwarf,
    unit: &Unit,
    offset: UnitOffset,
    prefix: &str,
    base: u64,
    depth: usize,
    out: &mut HashMap<String, Field>,
) -> gimli::Result<()> {
    if depth > DEPTH_MAX {
        return Ok(());
    }
    let mut tree = unit.entries_tree(Some(offset))?;
    let mut members = tree.root()?.children();
    while let Some(child) = members.next()? {
        let member = child.entry();
        // Bit fields share bytes, and static members are not stored in the struct.
        if member.tag() != gimli::DW_TAG_member
            || member.has_attr(gimli::DW_AT_bit_size)
            || member.has_attr(gimli::DW_AT_declaration)
        {
            continue;
        }
        let (Some(of), Some(at)) = (type_of(member), member_offset(unit, member)?) else {
            continue;
        };
        let at = base + at;
        let name = name_of(dwarf, unit, member)?;
        let path = match (&name, prefix.is_empty()) {
            (Some(name), true) => name.clone(),
            (Some(name), false) => format!("{prefix}.{name}"),
            (None, _) => prefix.to_string(),
        };
        if name.is_some() {
            let size = size_of(unit, of, 0)?.unwrap_or(0);
            out.entry(path.clone())
                .or_insert(Field { offset: at, size });
        }
        let inner = strip(unit, of)?;
        let entry = unit.entry(inner)?;
        match entry.tag() {
            gimli::DW_TAG_structure_type | gimli::DW_TAG_union_type => {
                flatten(dwarf, unit, inner, &path, at, depth + 1, out)?;
            }
            gimli::DW_TAG_array_type => {
                let Some(element) = type_of(&entry) else {
                    continue;
                };
                if let Some(stride) = size_of(unit, element, 0)? {
                    out.entry(format!("{path}[]")).or_insert(Field {
                        offset: at,
                        size: stride,
                    });
                }
                if let Some(element) = defined_struct(unit, element)? {
                    flatten(
                        dwarf,
                        unit,
                        element,
                        &format!("{path}[0]"),
                        at,
                        depth + 1,
                        out,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::test_dwarf::Firmware;

    #[test]
    fn structs_are_laid_out_by_path_through_typedefs_and_anonymous_unions() {
        let mut firmware = Firmware::new();
        let uchar = firmware.base("unsigned char", gimli::DW_ATE_unsigned_char, 1);
        let schar = firmware.base("signed char", gimli::DW_ATE_signed_char, 1);
        let uint = firmware.base("unsigned int", gimli::DW_ATE_unsigned, 4);
        let prio = firmware.structure("", 2, &[("prio", schar, 0), ("sched_locked", uchar, 1)]);
        let union = firmware.structure("", 2, &[("", prio, 0)]);
        let base = firmware.structure(
            "_thread_base",
            8,
            &[("thread_state", uchar, 0), ("", union, 2)],
        );
        let name = firmware.array(uchar, 32);
        let thread = firmware.structure("k_thread", 48, &[("base", base, 0), ("name", name, 16)]);
        let pointer = firmware.pointer(Some(thread));
        let cpu = firmware.structure("_cpu", 12, &[("nested", uint, 0), ("current", pointer, 8)]);
        let cpus = firmware.array(cpu, 2);
        let kernel = firmware.structure(
            "z_kernel",
            32,
            &[("cpus", cpus, 0), ("threads", pointer, 24)],
        );
        let typedef = firmware.typedef("kernel_t", kernel);
        let root = firmware.root();
        firmware.variable(root, "_kernel", typedef, 0x2000_0000);
        firmware.variable(root, "z_main_thread", thread, 0x2000_0100);
        let idle = firmware.array(thread, 2);
        firmware.variable(root, "z_idle_threads", idle, 0x2000_0200);

        let elf = firmware.elf(&[("_kernel", 0x2000_0000, 32), ("xFree.0", 0x2000_0400, 4)]);
        let debug = read(&elf, &["k_thread", "kernel_t"], &["k_thread"]).unwrap();
        let thread = &debug.structs["k_thread"];
        assert_eq!(thread.size, 48);
        assert_eq!(
            thread.field("base.thread_state"),
            Some(Field { offset: 0, size: 1 })
        );
        assert_eq!(
            thread.field("base.prio"),
            Some(Field { offset: 2, size: 1 })
        );
        assert_eq!(
            thread.field("name"),
            Some(Field {
                offset: 16,
                size: 32
            })
        );
        let kernel = &debug.structs["kernel_t"];
        assert_eq!(
            kernel.field("cpus[0].current"),
            Some(Field { offset: 8, size: 4 })
        );
        assert_eq!(
            kernel.field("cpus[]"),
            Some(Field {
                offset: 0,
                size: 12
            })
        );
        assert_eq!(
            kernel.field("threads"),
            Some(Field {
                offset: 24,
                size: 4
            })
        );
        assert_eq!(debug.data["_kernel"], (0x2000_0000, 32));
        // A static the linker renamed is under its own name too.
        assert_eq!(debug.data["xFree"], (0x2000_0400, 4));
        assert_eq!(
            debug.instances["k_thread"],
            [
                ("z_main_thread".to_string(), 0x2000_0100),
                ("z_idle_threads[0]".to_string(), 0x2000_0200),
                ("z_idle_threads[1]".to_string(), 0x2000_0230),
            ]
        );
    }
}
