//! Where an address is in a firmware's code, from its ELF file: the function,
//! the source file and the line. It turns the addresses of a crash into
//! something to read.
//!
//! Functions come from the symbol table, so a file without debug information
//! still names them. The debug information adds the lines, and the names as
//! they were written where the symbols are mangled, as in Rust and C++.

use std::borrow::Cow;
use std::collections::HashMap;

use gimli::{AttributeValue, EndianSlice, RunTimeEndian};
use object::{Architecture, Object, ObjectSection, ObjectSymbol, SymbolKind};
use serde::Serialize;

/// Where an address is in the code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CodePlace {
    pub function: String,
    /// The source file with the folder it is in, without debug information none.
    pub file: Option<String>,
    pub line: Option<u32>,
}

struct Function {
    start: u64,
    end: u64,
    name: String,
}

/// One row of the line tables: the code from this address on is of this line.
struct Row {
    address: u64,
    /// In `CodeMap::files`.
    file: usize,
    line: u32,
    /// The code before this address ended here: nothing is known from it on.
    end: bool,
}

/// A firmware's code, to look addresses up in.
pub struct CodeMap {
    /// By address.
    functions: Vec<Function>,
    /// By address.
    rows: Vec<Row>,
    files: Vec<String>,
    /// ARM code in Thumb mode has bit 0 set in its addresses, which is not part of the address.
    thumb: bool,
}

impl CodeMap {
    /// Reads an ELF file. It needs its symbols; the debug information is a bonus.
    pub fn new(data: &[u8]) -> Result<Self, String> {
        let file =
            object::File::parse(data).map_err(|error| format!("Not an ELF file: {error}"))?;
        let thumb = file.architecture() == Architecture::Arm;
        let mut functions = functions_in(&file, thumb);
        if functions.is_empty() {
            return Err("The ELF file has no functions in it: was it stripped?".into());
        }
        let mut names = HashMap::new();
        let mut rows = Vec::new();
        let mut files = Vec::new();
        // Debug information that cannot be read costs the lines, not the functions.
        let flash_at_zero = functions[0].start == 0;
        if read_dwarf(
            &file,
            thumb,
            flash_at_zero,
            &mut names,
            &mut rows,
            &mut files,
        )
        .is_err()
        {
            rows.clear();
        }
        for function in &mut functions {
            if let Some(name) = names.remove(&function.start) {
                function.name = name;
            }
        }
        // Where one run of code ends and another starts, the new one counts.
        rows.sort_by_key(|row| (row.address, !row.end));
        Ok(Self {
            functions,
            rows,
            files,
            thumb,
        })
    }

    /// Where an address is, if it is in a function: anything else is not code.
    pub fn place(&self, address: u64) -> Option<CodePlace> {
        let mut at = address;
        if self.thumb {
            at &= !1;
            // Bit 0 set and not a function's start, as in LR: a return address.
            // The call before it is what the line should be of.
            if address & 1 == 1
                && self
                    .function_at(at)
                    .is_none_or(|function| function.start != at)
            {
                at = at.saturating_sub(1);
            }
        }
        let function = self.function_at(at)?;
        let row = self
            .rows
            .partition_point(|row| row.address <= at)
            .checked_sub(1)
            .map(|index| &self.rows[index])
            .filter(|row| !row.end);
        Some(CodePlace {
            function: function.name.clone(),
            file: row.map(|row| self.files[row.file].clone()),
            line: row.map(|row| row.line).filter(|line| *line != 0),
        })
    }

    fn function_at(&self, at: u64) -> Option<&Function> {
        let index = self
            .functions
            .partition_point(|function| function.start <= at);
        self.functions[..index]
            .last()
            .filter(|function| at < function.end)
    }
}

/// The functions in the symbol table, by address, the first name of each.
fn functions_in(file: &object::File, thumb: bool) -> Vec<Function> {
    let mask = if thumb { !1 } else { !0 };
    let mut functions: Vec<Function> = file
        .symbols()
        .filter(|symbol| {
            symbol.kind() == SymbolKind::Text && symbol.is_definition() && symbol.size() > 0
        })
        .filter_map(|symbol| {
            let start = symbol.address() & mask;
            Some(Function {
                start,
                end: start + symbol.size(),
                name: symbol.name().ok()?.to_string(),
            })
        })
        .collect();
    functions.sort_by_key(|function| function.start);
    functions.dedup_by_key(|function| function.start);
    functions
}

type Slice<'a> = EndianSlice<'a, RunTimeEndian>;

/// Reads the names of the functions and the line tables. `flash_at_zero`: a
/// linker leaves the lines of code it dropped at address 0, which is only real
/// code where the flash is there.
fn read_dwarf(
    file: &object::File,
    thumb: bool,
    flash_at_zero: bool,
    names: &mut HashMap<u64, String>,
    rows: &mut Vec<Row>,
    files: &mut Vec<String>,
) -> gimli::Result<()> {
    let endian = if file.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };
    let sections = gimli::DwarfSections::load(|id| -> gimli::Result<Cow<[u8]>> {
        Ok(file
            .section_by_name(id.name())
            .and_then(|section| section.uncompressed_data().ok())
            .unwrap_or(Cow::Borrowed(&[])))
    })?;
    let dwarf: gimli::Dwarf<Slice> = sections.borrow(|section| EndianSlice::new(section, endian));
    let mask = if thumb { !1 } else { !0 };

    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;

        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            if entry.tag() != gimli::DW_TAG_subprogram {
                continue;
            }
            let Some(low) = entry.attr_value(gimli::DW_AT_low_pc) else {
                continue;
            };
            if let (Some(start), Some(name)) = (
                dwarf.attr_address(&unit, low)?,
                name_of(&dwarf, &unit, entry)?,
            ) {
                names.entry(start & mask).or_insert(name);
            }
        }

        let Some(program) = unit.line_program.clone() else {
            continue;
        };
        let mut known = HashMap::new();
        let mut lines = program.rows();
        let mut starting = true;
        let mut dropped = false;
        while let Some((header, row)) = lines.next_row()? {
            let address = row.address() & mask;
            if starting {
                dropped = address == 0 && !flash_at_zero;
                starting = false;
            }
            if row.end_sequence() {
                starting = true;
            }
            if dropped {
                continue;
            }
            let number = row.file_index();
            let index = match known.get(&number) {
                Some(index) => *index,
                None => {
                    let path = row
                        .file(header)
                        .map(|file| -> gimli::Result<String> {
                            let name = dwarf.attr_string(&unit, file.path_name())?;
                            let folder = match file.directory(header) {
                                Some(folder) => dwarf
                                    .attr_string(&unit, folder)?
                                    .to_string_lossy()
                                    .into_owned(),
                                None => String::new(),
                            };
                            Ok(short_path(&folder, &name.to_string_lossy()))
                        })
                        .transpose()?
                        .unwrap_or_default();
                    files.push(path);
                    known.insert(number, files.len() - 1);
                    files.len() - 1
                }
            };
            rows.push(Row {
                address,
                file: index,
                line: row.line().map_or(0, |line| line.get() as u32),
                end: row.end_sequence(),
            });
        }
    }
    Ok(())
}

/// A function's name; one made from an inline or a declaration has it there.
fn name_of(
    dwarf: &gimli::Dwarf<Slice>,
    unit: &gimli::Unit<Slice>,
    entry: &gimli::DebuggingInformationEntry<Slice>,
) -> gimli::Result<Option<String>> {
    if let Some(value) = entry.attr_value(gimli::DW_AT_name) {
        return Ok(Some(
            dwarf
                .attr_string(unit, value)?
                .to_string_lossy()
                .into_owned(),
        ));
    }
    for attribute in [gimli::DW_AT_abstract_origin, gimli::DW_AT_specification] {
        if let Some(AttributeValue::UnitRef(offset)) = entry.attr_value(attribute) {
            let origin = unit.entry(offset)?;
            if let Some(value) = origin.attr_value(gimli::DW_AT_name) {
                return Ok(Some(
                    dwarf
                        .attr_string(unit, value)?
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
            for attribute in [gimli::DW_AT_abstract_origin, gimli::DW_AT_specification] {
                if let Some(AttributeValue::UnitRef(offset)) = origin.attr_value(attribute)
                    && let Some(value) = unit.entry(offset)?.attr_value(gimli::DW_AT_name)
                {
                    return Ok(Some(
                        dwarf
                            .attr_string(unit, value)?
                            .to_string_lossy()
                            .into_owned(),
                    ));
                }
            }
        }
    }
    Ok(None)
}

/// A source file with the folder it is in, enough to know it: `src/main.c`.
fn short_path(folder: &str, name: &str) -> String {
    let joined = if name.contains(['/', '\\']) || folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    };
    let mut parts = joined.rsplit(['/', '\\']).filter(|part| !part.is_empty());
    match (parts.next(), parts.next()) {
        (Some(file), Some(parent)) => format!("{parent}/{file}"),
        (Some(file), None) => file.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use gimli::write::{
        Address, AttributeValue as Value, DwarfUnit, EndianVec, LineProgram, LineString, Sections,
    };
    use gimli::{Encoding, Format, LineEncoding, LittleEndian};
    use object::write::{Object as Writer, Symbol, SymbolSection};
    use object::{BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolScope};

    use super::*;

    const ENCODING: Encoding = Encoding {
        address_size: 4,
        format: Format::Dwarf32,
        version: 4,
    };

    /// `.debug_*` sections with one function, its name as written, and a line table for it:
    /// line 10 from 0x08000100, line 11 from 0x08000110, ending at 0x08000120.
    fn debug_info() -> Vec<(&'static str, Vec<u8>)> {
        let mut dwarf = DwarfUnit::new(ENCODING);
        let folder = LineString::String(b"C:/project/src".to_vec());
        let file = LineString::String(b"sensor.c".to_vec());
        let mut program = LineProgram::new(
            ENCODING,
            LineEncoding::default(),
            folder.clone(),
            None,
            file.clone(),
            None,
        );
        let directory = program.default_directory();
        let file_id = program.add_file(file, directory, None);
        program.begin_sequence(Some(Address::Constant(0x0800_0100)));
        program.row().file = file_id;
        program.row().line = 10;
        program.generate_row();
        program.row().address_offset = 0x10;
        program.row().line = 11;
        program.generate_row();
        program.end_sequence(0x20);
        dwarf.unit.line_program = program;

        let root = dwarf.unit.root();
        dwarf.unit.get_mut(root).set(
            gimli::DW_AT_comp_dir,
            Value::String(b"C:/project/src".to_vec()),
        );
        let function = dwarf.unit.add(root, gimli::DW_TAG_subprogram);
        let entry = dwarf.unit.get_mut(function);
        entry.set(gimli::DW_AT_name, Value::String(b"sensor_read".to_vec()));
        entry.set(
            gimli::DW_AT_low_pc,
            Value::Address(Address::Constant(0x0800_0100)),
        );
        entry.set(gimli::DW_AT_high_pc, Value::Udata(0x20));

        let mut sections = Sections::new(EndianVec::new(LittleEndian));
        dwarf.write(&mut sections).unwrap();
        let mut out = Vec::new();
        sections
            .for_each(|id, data| -> Result<(), ()> {
                if !data.slice().is_empty() {
                    out.push((id.name(), data.slice().to_vec()));
                }
                Ok(())
            })
            .unwrap();
        out
    }

    /// An ARM ELF file with a mangled `sensor_read` at 0x08000100 and `other` after it.
    fn firmware(with_debug: bool) -> Vec<u8> {
        let mut writer = Writer::new(
            BinaryFormat::Elf,
            object::Architecture::Arm,
            Endianness::Little,
        );
        let text = writer.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
        writer.append_section_data(text, &[0; 0x40], 4);
        for (name, value, size) in [
            ("_ZN6sensor4read17hE", 0x0800_0101, 0x20),
            ("other", 0x0800_0121, 0x10),
        ] {
            writer.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value,
                size,
                kind: SymbolKind::Text,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(text),
                flags: SymbolFlags::None,
            });
        }
        if with_debug {
            for (name, data) in debug_info() {
                let section =
                    writer.add_section(Vec::new(), name.as_bytes().to_vec(), SectionKind::Debug);
                writer.append_section_data(section, &data, 1);
            }
        }
        writer.write().unwrap()
    }

    #[test]
    fn finds_the_function_file_and_line() {
        let map = CodeMap::new(&firmware(true)).unwrap();
        assert_eq!(
            map.place(0x0800_0112),
            Some(CodePlace {
                function: "sensor_read".into(),
                file: Some("src/sensor.c".into()),
                line: Some(11),
            })
        );
        assert_eq!(map.place(0x0800_0100).unwrap().line, Some(10));
    }

    #[test]
    fn a_return_address_is_of_the_call_before_it() {
        let map = CodeMap::new(&firmware(true)).unwrap();
        // LR, with its Thumb bit: the call was just before line 11 began.
        assert_eq!(map.place(0x0800_0111).unwrap().line, Some(10));
    }

    #[test]
    fn a_pointer_to_a_function_is_that_function() {
        let map = CodeMap::new(&firmware(true)).unwrap();
        assert_eq!(map.place(0x0800_0121).unwrap().function, "other");
        assert_eq!(map.place(0x0800_0101).unwrap().function, "sensor_read");
    }

    #[test]
    fn what_is_not_in_a_function_is_not_code() {
        let map = CodeMap::new(&firmware(true)).unwrap();
        assert_eq!(map.place(0x2000_0C40), None);
        assert_eq!(map.place(0x0800_00FF), None);
        assert_eq!(map.place(0x0800_0130), None);
        assert_eq!(map.place(0), None);
    }

    #[test]
    fn without_debug_information_the_symbols_still_name_it() {
        let map = CodeMap::new(&firmware(false)).unwrap();
        assert_eq!(
            map.place(0x0800_0122),
            Some(CodePlace {
                function: "other".into(),
                file: None,
                line: None,
            })
        );
    }

    #[test]
    fn a_stripped_file_says_so() {
        let writer = Writer::new(
            BinaryFormat::Elf,
            object::Architecture::Arm,
            Endianness::Little,
        );
        let error = CodeMap::new(&writer.write().unwrap()).err().unwrap();
        assert!(error.contains("stripped"), "{error}");
        assert!(CodeMap::new(b"not an elf").is_err());
    }

    #[test]
    fn a_path_keeps_the_folder_it_is_in() {
        assert_eq!(short_path("C:/project/src", "main.c"), "src/main.c");
        assert_eq!(
            short_path("/home/me", "zephyr/kernel/sched.c"),
            "kernel/sched.c"
        );
        assert_eq!(short_path("", "main.c"), "main.c");
        assert_eq!(short_path("C:\\project\\src", "main.c"), "src/main.c");
    }
}
