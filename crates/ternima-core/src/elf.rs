//! The variables in a firmware's ELF file, from its DWARF debug information:
//! globals and statics, the members of structs and the elements of small
//! arrays, each with its address and how to read it. Without debug
//! information, the symbol table's data, read as unsigned numbers.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use gimli::{AttributeValue, DwAte, EndianSlice, RunTimeEndian, UnitOffset};
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use serde::{Deserialize, Serialize};

/// The most elements of an array opened, each as a variable of its own.
const ARRAY_MAX: u64 = 32;
/// How deep structs in structs, and arrays in them, are opened.
const DEPTH_MAX: usize = 6;
/// The most variables one global opens into.
const PARTS_MAX: usize = 256;

/// How a variable's bytes are read: little-endian, as on Cortex-M and RISC-V.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

impl Kind {
    /// How many bytes it takes.
    pub fn size(self) -> usize {
        match self {
            Kind::Bool | Kind::U8 | Kind::I8 => 1,
            Kind::U16 | Kind::I16 => 2,
            Kind::U32 | Kind::I32 | Kind::F32 => 4,
            Kind::U64 | Kind::I64 | Kind::F64 => 8,
        }
    }

    /// The value of the bytes read from the chip; `bytes` holds `size()` of them.
    pub fn decode(self, bytes: &[u8]) -> f64 {
        let mut word = [0; 8];
        word[..self.size()].copy_from_slice(&bytes[..self.size()]);
        let value = u64::from_le_bytes(word);
        match self {
            Kind::Bool => f64::from(u8::from(value != 0)),
            Kind::U8 => value as u8 as f64,
            Kind::I8 => value as u8 as i8 as f64,
            Kind::U16 => value as u16 as f64,
            Kind::I16 => value as u16 as i16 as f64,
            Kind::U32 => value as u32 as f64,
            Kind::I32 => value as u32 as i32 as f64,
            Kind::U64 => value as f64,
            Kind::I64 => value as i64 as f64,
            Kind::F32 => f64::from(f32::from_bits(value as u32)),
            Kind::F64 => f64::from_bits(value),
        }
    }

    fn unsigned(size: u64) -> Option<Kind> {
        match size {
            1 => Some(Kind::U8),
            2 => Some(Kind::U16),
            4 => Some(Kind::U32),
            8 => Some(Kind::U64),
            _ => None,
        }
    }

    fn signed(size: u64) -> Option<Kind> {
        match size {
            1 => Some(Kind::I8),
            2 => Some(Kind::I16),
            4 => Some(Kind::I32),
            8 => Some(Kind::I64),
            _ => None,
        }
    }

    /// A base type's, from its DWARF encoding and size.
    fn of(encoding: DwAte, size: u64) -> Option<Kind> {
        match encoding {
            gimli::DW_ATE_boolean if size == 1 => Some(Kind::Bool),
            gimli::DW_ATE_boolean => Kind::unsigned(size),
            gimli::DW_ATE_float => match size {
                4 => Some(Kind::F32),
                8 => Some(Kind::F64),
                _ => None,
            },
            gimli::DW_ATE_signed | gimli::DW_ATE_signed_char => Kind::signed(size),
            gimli::DW_ATE_unsigned | gimli::DW_ATE_unsigned_char | gimli::DW_ATE_UTF => {
                Kind::unsigned(size)
            }
            _ => None,
        }
    }
}

/// A variable that can be read from the chip as it runs: a global or a
/// static, a member of one, or an element of an array.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Variable {
    /// As the source names it: `counter`, `sensor.temp`, `samples[2]`, or
    /// `app::COUNTER` inside a namespace or a module. Statics of one name in
    /// two source files are named after them as GDB does, `'uart.c'::state`.
    pub name: String,
    pub address: u64,
    pub kind: Kind,
    /// Its type as the source names it, such as `uint16_t`, `float` or `enum mode`.
    pub type_name: String,
}

/// Every variable in an ELF file, by name.
pub fn variables(data: &[u8]) -> Result<Vec<Variable>, String> {
    let file =
        object::File::parse(data).map_err(|error| format!("It is not an ELF file: {error}"))?;
    let mut found = from_dwarf(&file)
        .map_err(|error| format!("Its debug information could not be read: {error}"))?;
    if found.is_empty() {
        found = from_symbols(&file);
    }
    let mut found = named_apart(found);
    found.sort_by_cached_key(|variable| natural(&variable.name));
    Ok(found)
}

/// Gives variables of one name, each with the source file it is in, names
/// of their own. Those in different files are named after them as GDB names
/// them, `'uart.c'::state` and `'spi.c'::state`; those still of one name,
/// in one file, are numbered: `poll::count (2)` after `poll::count`.
fn named_apart(found: Vec<(Variable, String)>) -> Vec<Variable> {
    let mut files: HashMap<String, HashSet<String>> = HashMap::new();
    for (variable, file) in &found {
        files
            .entry(variable.name.clone())
            .or_default()
            .insert(file.clone());
    }
    let mut named: Vec<Variable> = found
        .into_iter()
        .map(|(mut variable, file)| {
            if files[&variable.name].len() > 1 && !file.is_empty() {
                variable.name = format!("'{file}'::{}", variable.name);
            }
            variable
        })
        .collect();
    let mut taken: HashMap<String, usize> = HashMap::new();
    for variable in &mut named {
        let count = taken.entry(variable.name.clone()).or_default();
        *count += 1;
        if *count > 1 {
            variable.name = format!("{} ({count})", variable.name);
        }
    }
    named
}

/// A path's last part: `main.c` of `../Core/Src/main.c` or of `C:\src\main.c`.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A name for sorting with its numbers by value, so samples[2] comes before samples[10].
fn natural(name: &str) -> Vec<(String, u64)> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut number = u64::from(c as u8 - b'0');
            while let Some(digit) = chars.peek().and_then(|next| next.to_digit(10)) {
                number = number.saturating_mul(10).saturating_add(u64::from(digit));
                chars.next();
            }
            parts.push((std::mem::take(&mut text), number));
        } else {
            text.push(c);
        }
    }
    parts.push((text, 0));
    parts
}

type Slice<'a> = EndianSlice<'a, RunTimeEndian>;
type Dwarf<'a> = gimli::Dwarf<Slice<'a>>;
type Unit<'a> = gimli::Unit<Slice<'a>>;
type Entry<'a> = gimli::DebuggingInformationEntry<Slice<'a>>;

/// The variables in the debug information, each with the source file of its unit.
fn from_dwarf(file: &object::File) -> gimli::Result<Vec<(Variable, String)>> {
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
    let dwarf = sections.borrow(|section| EndianSlice::new(section, endian));

    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let source = unit
            .name
            .map(|name| file_name(&name.to_string_lossy()).to_string())
            .unwrap_or_default();
        // The namespaces, modules and functions around an entry, with their depths.
        let mut scopes: Vec<(isize, String)> = Vec::new();
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            let depth = entry.depth();
            while scopes.last().is_some_and(|(at, _)| *at >= depth) {
                scopes.pop();
            }
            match entry.tag() {
                gimli::DW_TAG_namespace
                | gimli::DW_TAG_subprogram
                | gimli::DW_TAG_structure_type
                | gimli::DW_TAG_class_type
                | gimli::DW_TAG_union_type => {
                    if entry.has_children() {
                        let name = name_of(&dwarf, &unit, entry)?.unwrap_or_default();
                        scopes.push((depth, name));
                    }
                }
                gimli::DW_TAG_variable => {
                    let mut parts = Vec::new();
                    global(&dwarf, &unit, entry, &scopes, &mut parts)?;
                    for part in parts {
                        if seen.insert((part.name.clone(), part.address)) {
                            found.push((part, source.clone()));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

/// A variable with a fixed address, opened into the parts that can be read.
fn global(
    dwarf: &Dwarf,
    unit: &Unit,
    entry: &Entry,
    scopes: &[(isize, String)],
    out: &mut Vec<Variable>,
) -> gimli::Result<()> {
    let Some(address) = fixed_address(dwarf, unit, entry)? else {
        return Ok(());
    };
    // A definition outside its class or namespace has its name and type in the declaration.
    let declaration = match entry.attr_value(gimli::DW_AT_specification) {
        Some(AttributeValue::UnitRef(offset)) => Some(unit.entry(offset)?),
        _ => None,
    };
    let name = match name_of(dwarf, unit, entry)? {
        Some(name) => Some(name),
        None => match &declaration {
            Some(declaration) => name_of(dwarf, unit, declaration)?,
            None => None,
        },
    };
    let type_offset = type_of(entry).or_else(|| {
        declaration
            .as_ref()
            .and_then(|declaration| type_of(declaration))
    });
    let (Some(name), Some(type_offset)) = (name, type_offset) else {
        return Ok(());
    };
    // Vtables and the like, which the compiler made.
    if name.contains('{') || name.contains('<') || entry.has_attr(gimli::DW_AT_artificial) {
        return Ok(());
    }
    let full = scopes
        .iter()
        .map(|(_, scope)| scope.as_str())
        .filter(|scope| !scope.is_empty())
        .chain([name.as_str()])
        .collect::<Vec<_>>()
        .join("::");
    open(dwarf, unit, type_offset, &full, address, None, 0, out)
}

/// The address of a variable that has one for the whole run: not one on the
/// stack, in a register, or local to a thread. Globals the linker merged are
/// an address and an offset into it.
fn fixed_address(dwarf: &Dwarf, unit: &Unit, entry: &Entry) -> gimli::Result<Option<u64>> {
    let Some(AttributeValue::Exprloc(expression)) = entry.attr_value(gimli::DW_AT_location) else {
        return Ok(None);
    };
    let mut operations = expression.operations(unit.encoding());
    let mut address = match operations.next()? {
        Some(gimli::Operation::Address { address }) => address,
        Some(gimli::Operation::AddressIndex { index }) => dwarf.address(unit, index)?,
        _ => return Ok(None),
    };
    while let Some(operation) = operations.next()? {
        match operation {
            gimli::Operation::PlusConstant { value } => address = address.wrapping_add(value),
            _ => return Ok(None),
        }
    }
    Ok(Some(address))
}

/// Opens a type into the variables that can be read: a number as it is, a
/// struct into its members, an array into its first elements.
#[allow(clippy::too_many_arguments)]
fn open(
    dwarf: &Dwarf,
    unit: &Unit,
    offset: UnitOffset,
    name: &str,
    address: u64,
    type_name: Option<String>,
    depth: usize,
    out: &mut Vec<Variable>,
) -> gimli::Result<()> {
    if depth > DEPTH_MAX || out.len() >= PARTS_MAX {
        return Ok(());
    }
    let entry = unit.entry(offset)?;
    let own_name = name_of(dwarf, unit, &entry)?;
    let mut push = |kind: Kind, type_name: String| {
        out.push(Variable {
            name: name.to_string(),
            address,
            kind,
            type_name,
        });
    };
    match entry.tag() {
        // The first name a type is given, such as uint32_t, is the one shown.
        gimli::DW_TAG_typedef => {
            if let Some(next) = type_of(&entry) {
                open(
                    dwarf,
                    unit,
                    next,
                    name,
                    address,
                    type_name.or(own_name),
                    depth,
                    out,
                )?;
            }
        }
        gimli::DW_TAG_const_type
        | gimli::DW_TAG_volatile_type
        | gimli::DW_TAG_atomic_type
        | gimli::DW_TAG_restrict_type => {
            if let Some(next) = type_of(&entry) {
                open(dwarf, unit, next, name, address, type_name, depth, out)?;
            }
        }
        gimli::DW_TAG_base_type => {
            let size = byte_size(&entry).unwrap_or(0);
            if let Some(AttributeValue::Encoding(encoding)) =
                entry.attr_value(gimli::DW_AT_encoding)
                && let Some(kind) = Kind::of(encoding, size)
            {
                push(kind, type_name.or(own_name).unwrap_or_default());
            }
        }
        gimli::DW_TAG_enumeration_type => {
            let size = byte_size(&entry).unwrap_or(4);
            let signed = match type_of(&entry) {
                Some(base) => matches!(
                    unit.entry(base)?.attr_value(gimli::DW_AT_encoding),
                    Some(AttributeValue::Encoding(
                        gimli::DW_ATE_signed | gimli::DW_ATE_signed_char
                    ))
                ),
                None => false,
            };
            let kind = if signed {
                Kind::signed(size)
            } else {
                Kind::unsigned(size)
            };
            if let Some(kind) = kind {
                let shown =
                    own_name.map_or_else(|| "enum".to_string(), |own| format!("enum {own}"));
                push(kind, type_name.unwrap_or(shown));
            }
        }
        gimli::DW_TAG_pointer_type
        | gimli::DW_TAG_reference_type
        | gimli::DW_TAG_rvalue_reference_type => {
            let size = byte_size(&entry).unwrap_or(u64::from(unit.encoding().address_size));
            if let Some(kind) = Kind::unsigned(size) {
                push(kind, type_name.unwrap_or_else(|| "pointer".into()));
            }
        }
        gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type | gimli::DW_TAG_union_type => {
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
                let Some(member_type) = type_of(member) else {
                    continue;
                };
                let Some(at) = member_offset(unit, member)? else {
                    continue;
                };
                // An anonymous struct or union inside lends its members to this one.
                let member_name = match name_of(dwarf, unit, member)? {
                    Some(member_name) => format!("{name}.{member_name}"),
                    None => name.to_string(),
                };
                open(
                    dwarf,
                    unit,
                    member_type,
                    &member_name,
                    address + at,
                    None,
                    depth + 1,
                    out,
                )?;
            }
        }
        gimli::DW_TAG_array_type => {
            let Some(element) = type_of(&entry) else {
                return Ok(());
            };
            let Some(count) = array_length(unit, offset)? else {
                return Ok(());
            };
            let Some(stride) = size_of(unit, element, 0)? else {
                return Ok(());
            };
            for index in 0..count.min(ARRAY_MAX) {
                let element_name = format!("{name}[{index}]");
                open(
                    dwarf,
                    unit,
                    element,
                    &element_name,
                    address + index * stride,
                    None,
                    depth + 1,
                    out,
                )?;
            }
        }
        // Functions, Rust enums with data, and the like are not read.
        _ => {}
    }
    Ok(())
}

/// How many elements a one-dimensional array has; `None` for an array of
/// arrays, or one of no fixed length.
fn array_length(unit: &Unit, offset: UnitOffset) -> gimli::Result<Option<u64>> {
    let mut tree = unit.entries_tree(Some(offset))?;
    let mut ranges = tree.root()?.children();
    let mut length = None;
    while let Some(child) = ranges.next()? {
        let range = child.entry();
        if range.tag() != gimli::DW_TAG_subrange_type {
            continue;
        }
        if length.is_some() {
            return Ok(None);
        }
        let count = range
            .attr_value(gimli::DW_AT_count)
            .and_then(|count| count.udata_value());
        let upper = range
            .attr_value(gimli::DW_AT_upper_bound)
            .and_then(|upper| upper.udata_value());
        let lower = range
            .attr_value(gimli::DW_AT_lower_bound)
            .and_then(|lower| lower.udata_value())
            .unwrap_or(0);
        length = count.or(upper.map(|upper| upper + 1 - lower));
        if length.is_none() {
            return Ok(None);
        }
    }
    Ok(length)
}

/// How many bytes a type takes.
fn size_of(unit: &Unit, offset: UnitOffset, depth: usize) -> gimli::Result<Option<u64>> {
    if depth > DEPTH_MAX * 4 {
        return Ok(None);
    }
    let entry = unit.entry(offset)?;
    if let Some(size) = byte_size(&entry) {
        return Ok(Some(size));
    }
    match entry.tag() {
        gimli::DW_TAG_typedef
        | gimli::DW_TAG_const_type
        | gimli::DW_TAG_volatile_type
        | gimli::DW_TAG_atomic_type
        | gimli::DW_TAG_restrict_type => match type_of(&entry) {
            Some(next) => size_of(unit, next, depth + 1),
            None => Ok(None),
        },
        gimli::DW_TAG_pointer_type => Ok(Some(u64::from(unit.encoding().address_size))),
        gimli::DW_TAG_array_type => {
            let (Some(element), Some(count)) = (type_of(&entry), array_length(unit, offset)?)
            else {
                return Ok(None);
            };
            Ok(size_of(unit, element, depth + 1)?.map(|size| size * count))
        }
        _ => Ok(None),
    }
}

/// Where a member is in its struct: 0 in a union, which gives none.
fn member_offset(unit: &Unit, member: &Entry) -> gimli::Result<Option<u64>> {
    match member.attr_value(gimli::DW_AT_data_member_location) {
        None => Ok(Some(0)),
        Some(AttributeValue::Exprloc(expression)) => {
            let mut operations = expression.operations(unit.encoding());
            match operations.next()? {
                Some(gimli::Operation::PlusConstant { value }) => Ok(Some(value)),
                _ => Ok(None),
            }
        }
        Some(value) => Ok(value.udata_value()),
    }
}

fn byte_size(entry: &Entry) -> Option<u64> {
    entry
        .attr_value(gimli::DW_AT_byte_size)
        .and_then(|size| size.udata_value())
}

fn type_of(entry: &Entry) -> Option<UnitOffset> {
    match entry.attr_value(gimli::DW_AT_type) {
        Some(AttributeValue::UnitRef(offset)) => Some(offset),
        _ => None,
    }
}

fn name_of(dwarf: &Dwarf, unit: &Unit, entry: &Entry) -> gimli::Result<Option<String>> {
    match entry.attr_value(gimli::DW_AT_name) {
        Some(value) => Ok(Some(
            dwarf
                .attr_string(unit, value)?
                .to_string_lossy()
                .into_owned(),
        )),
        None => Ok(None),
    }
}

/// Without debug information: the symbol table's data, by size, as unsigned
/// numbers. A static is of the source file whose symbol comes before it.
fn from_symbols(file: &object::File) -> Vec<(Variable, String)> {
    let mut source = String::new();
    let mut found = Vec::new();
    for symbol in file.symbols() {
        if symbol.kind() == SymbolKind::File {
            source = symbol.name().map(file_name).unwrap_or_default().to_string();
            continue;
        }
        if symbol.kind() != SymbolKind::Data || !symbol.is_definition() {
            continue;
        }
        let (Some(kind), Ok(name)) = (Kind::unsigned(symbol.size()), symbol.name()) else {
            continue;
        };
        let variable = Variable {
            name: name.to_string(),
            address: symbol.address(),
            kind,
            type_name: format!("{} bytes", symbol.size()),
        };
        // A global is the one of its name, where a static is of its file.
        let file = if symbol.is_local() {
            source.clone()
        } else {
            String::new()
        };
        found.push((variable, file));
    }
    found
}

#[cfg(test)]
mod tests {
    use gimli::write::{
        Address, AttributeValue as Value, EndianVec, Expression, LineProgram, Sections,
        UnitEntryId, UnitId,
    };
    use gimli::{Encoding, Format, LittleEndian};
    use object::write::{Object as Writer, Symbol, SymbolSection};
    use object::{Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolScope};

    use super::*;

    const ENCODING: Encoding = Encoding {
        format: Format::Dwarf32,
        version: 4,
        address_size: 4,
    };

    /// Builds DWARF for a firmware's variables, as a compiler would: a unit
    /// for each source file, what is added going in the last one.
    struct Firmware {
        dwarf: gimli::write::Dwarf,
        unit: UnitId,
    }

    impl Firmware {
        fn new() -> Self {
            let mut dwarf = gimli::write::Dwarf::new();
            let unit = dwarf
                .units
                .add(gimli::write::Unit::new(ENCODING, LineProgram::none()));
            Self { dwarf, unit }
        }

        /// A unit for another source file, compiled from `path`.
        fn source(&mut self, path: &str) {
            self.unit = self
                .dwarf
                .units
                .add(gimli::write::Unit::new(ENCODING, LineProgram::none()));
            let root = self.root();
            self.set(
                root,
                gimli::DW_AT_name,
                Value::String(path.as_bytes().to_vec()),
            );
        }

        fn root(&self) -> UnitEntryId {
            self.dwarf.units.get(self.unit).root()
        }

        fn add(
            &mut self,
            parent: UnitEntryId,
            tag: gimli::DwTag,
            name: Option<&str>,
        ) -> UnitEntryId {
            let id = self.dwarf.units.get_mut(self.unit).add(parent, tag);
            if let Some(name) = name {
                self.set(
                    id,
                    gimli::DW_AT_name,
                    Value::String(name.as_bytes().to_vec()),
                );
            }
            id
        }

        fn set(&mut self, id: UnitEntryId, name: gimli::DwAt, value: Value) {
            self.dwarf
                .units
                .get_mut(self.unit)
                .get_mut(id)
                .set(name, value);
        }

        fn base(&mut self, name: &str, encoding: DwAte, size: u64) -> UnitEntryId {
            let id = self.add(self.root(), gimli::DW_TAG_base_type, Some(name));
            self.set(id, gimli::DW_AT_encoding, Value::Encoding(encoding));
            self.set(id, gimli::DW_AT_byte_size, Value::Udata(size));
            id
        }

        fn typedef(&mut self, name: &str, of: UnitEntryId) -> UnitEntryId {
            let id = self.add(self.root(), gimli::DW_TAG_typedef, Some(name));
            self.set(id, gimli::DW_AT_type, Value::UnitRef(of));
            id
        }

        fn structure(
            &mut self,
            name: &str,
            size: u64,
            members: &[(&str, UnitEntryId, u64)],
        ) -> UnitEntryId {
            let id = self.add(self.root(), gimli::DW_TAG_structure_type, Some(name));
            self.set(id, gimli::DW_AT_byte_size, Value::Udata(size));
            for &(member, of, at) in members {
                let member = self.add(id, gimli::DW_TAG_member, Some(member));
                self.set(member, gimli::DW_AT_type, Value::UnitRef(of));
                self.set(member, gimli::DW_AT_data_member_location, Value::Udata(at));
            }
            id
        }

        fn array(&mut self, of: UnitEntryId, count: u64) -> UnitEntryId {
            let id = self.add(self.root(), gimli::DW_TAG_array_type, None);
            self.set(id, gimli::DW_AT_type, Value::UnitRef(of));
            let range = self.add(id, gimli::DW_TAG_subrange_type, None);
            self.set(range, gimli::DW_AT_count, Value::Udata(count));
            id
        }

        fn variable(
            &mut self,
            parent: UnitEntryId,
            name: &str,
            of: UnitEntryId,
            address: u64,
        ) -> UnitEntryId {
            let id = self.add(parent, gimli::DW_TAG_variable, Some(name));
            self.set(id, gimli::DW_AT_type, Value::UnitRef(of));
            let mut location = Expression::new();
            location.op_addr(Address::Constant(address));
            self.set(id, gimli::DW_AT_location, Value::Exprloc(location));
            id
        }

        /// The ELF file, with the DWARF sections and the symbols given.
        fn elf(mut self, symbols: &[(&str, u64, u64)]) -> Vec<u8> {
            let mut sections = Sections::new(EndianVec::new(LittleEndian));
            self.dwarf.write(&mut sections).unwrap();
            let mut elf = Writer::new(BinaryFormat::Elf, Architecture::Arm, Endianness::Little);
            sections
                .for_each(|id, data| -> Result<(), ()> {
                    if !data.slice().is_empty() {
                        let section = elf.add_section(
                            Vec::new(),
                            id.name().as_bytes().to_vec(),
                            SectionKind::Debug,
                        );
                        elf.append_section_data(section, data.slice(), 1);
                    }
                    Ok(())
                })
                .unwrap();
            // In RAM, as firmware keeps its data: a .bss from 0 that reaches the symbols.
            let bss = elf.add_section(Vec::new(), b".bss".to_vec(), SectionKind::UninitializedData);
            elf.append_section_bss(bss, 0x2000_1000, 4);
            for &(name, address, size) in symbols {
                elf.add_symbol(Symbol {
                    name: name.as_bytes().to_vec(),
                    value: address,
                    size,
                    kind: SymbolKind::Data,
                    scope: SymbolScope::Linkage,
                    weak: false,
                    section: SymbolSection::Section(bss),
                    flags: SymbolFlags::None,
                });
            }
            elf.write().unwrap()
        }
    }

    fn find<'a>(found: &'a [Variable], name: &str) -> &'a Variable {
        found
            .iter()
            .find(|variable| variable.name == name)
            .unwrap_or_else(|| panic!("no {name} in {found:#?}"))
    }

    #[test]
    fn globals_have_their_address_kind_and_type_name() {
        let mut firmware = Firmware::new();
        let uint = firmware.base("unsigned int", gimli::DW_ATE_unsigned, 4);
        let uint32 = firmware.typedef("uint32_t", uint);
        let schar = firmware.base("signed char", gimli::DW_ATE_signed_char, 1);
        let float = firmware.base("float", gimli::DW_ATE_float, 4);
        let double = firmware.base("double", gimli::DW_ATE_float, 8);
        let boolean = firmware.base("_Bool", gimli::DW_ATE_boolean, 1);
        let root = firmware.root();
        firmware.variable(root, "counter", uint32, 0x2000_0000);
        firmware.variable(root, "level", schar, 0x2000_0004);
        firmware.variable(root, "gain", float, 0x2000_0008);
        firmware.variable(root, "voltage", double, 0x2000_0010);
        firmware.variable(root, "ready", boolean, 0x2000_0018);

        let found = variables(&firmware.elf(&[])).unwrap();
        assert_eq!(
            find(&found, "counter"),
            &Variable {
                name: "counter".into(),
                address: 0x2000_0000,
                kind: Kind::U32,
                type_name: "uint32_t".into(),
            }
        );
        assert_eq!(find(&found, "level").kind, Kind::I8);
        assert_eq!(find(&found, "gain").kind, Kind::F32);
        assert_eq!(find(&found, "voltage").kind, Kind::F64);
        assert_eq!(find(&found, "ready").kind, Kind::Bool);
        // In order by name, for the search list.
        let names: Vec<_> = found
            .iter()
            .map(|variable| variable.name.as_str())
            .collect();
        assert_eq!(names, ["counter", "gain", "level", "ready", "voltage"]);
    }

    #[test]
    fn structs_open_into_members_and_arrays_into_elements() {
        let mut firmware = Firmware::new();
        let short = firmware.base("short", gimli::DW_ATE_signed, 2);
        let float = firmware.base("float", gimli::DW_ATE_float, 4);
        let ushort = firmware.base("unsigned short", gimli::DW_ATE_unsigned, 2);
        let point = firmware.structure("point", 4, &[("x", short, 0), ("y", short, 2)]);
        let samples = firmware.array(ushort, 4);
        let sensor = firmware.structure(
            "sensor",
            16,
            &[
                ("temp", float, 0),
                ("at", point, 4),
                ("samples", samples, 8),
            ],
        );
        let root = firmware.root();
        firmware.variable(root, "sensor", sensor, 0x2000_0100);

        let found = variables(&firmware.elf(&[])).unwrap();
        assert_eq!(find(&found, "sensor.temp").address, 0x2000_0100);
        assert_eq!(find(&found, "sensor.temp").kind, Kind::F32);
        assert_eq!(find(&found, "sensor.at.y").address, 0x2000_0106);
        assert_eq!(find(&found, "sensor.at.y").kind, Kind::I16);
        assert_eq!(find(&found, "sensor.samples[0]").address, 0x2000_0108);
        assert_eq!(find(&found, "sensor.samples[3]").address, 0x2000_010e);
        assert_eq!(found.len(), 7);
    }

    #[test]
    fn a_big_array_opens_its_first_elements() {
        let mut firmware = Firmware::new();
        let byte = firmware.base("unsigned char", gimli::DW_ATE_unsigned_char, 1);
        let buffer = firmware.array(byte, 1000);
        let root = firmware.root();
        firmware.variable(root, "rx", buffer, 0x2000_0400);

        let found = variables(&firmware.elf(&[])).unwrap();
        assert_eq!(found.len(), ARRAY_MAX as usize);
        assert_eq!(find(&found, "rx[31]").address, 0x2000_041f);
        // In order by index, not as text.
        assert_eq!(found[2].name, "rx[2]");
        assert_eq!(found[10].name, "rx[10]");
    }

    #[test]
    fn statics_in_namespaces_and_functions_are_named_after_them() {
        let mut firmware = Firmware::new();
        let uint = firmware.base("u32", gimli::DW_ATE_unsigned, 4);
        let root = firmware.root();
        let app = firmware.add(root, gimli::DW_TAG_namespace, Some("app"));
        let inner = firmware.add(app, gimli::DW_TAG_namespace, Some("inner"));
        firmware.variable(inner, "TICKS", uint, 0x2000_0020);
        let function = firmware.add(root, gimli::DW_TAG_subprogram, Some("poll"));
        firmware.variable(function, "calls", uint, 0x2000_0024);
        firmware.variable(root, "after", uint, 0x2000_0028);

        let found = variables(&firmware.elf(&[])).unwrap();
        assert_eq!(find(&found, "app::inner::TICKS").address, 0x2000_0020);
        assert_eq!(find(&found, "poll::calls").address, 0x2000_0024);
        // A namespace ends where its children do.
        assert_eq!(find(&found, "after").address, 0x2000_0028);
    }

    #[test]
    fn merged_globals_are_an_address_and_an_offset() {
        let mut firmware = Firmware::new();
        let uint = firmware.base("unsigned int", gimli::DW_ATE_unsigned, 4);
        let root = firmware.root();
        let merged = firmware.add(root, gimli::DW_TAG_variable, Some("uptime"));
        firmware.set(merged, gimli::DW_AT_type, Value::UnitRef(uint));
        let mut location = Expression::new();
        location.op_addr(Address::Constant(0x2000_0c60));
        location.op_plus_uconst(4);
        firmware.set(merged, gimli::DW_AT_location, Value::Exprloc(location));

        let found = variables(&firmware.elf(&[])).unwrap();
        assert_eq!(find(&found, "uptime").address, 0x2000_0c64);
    }

    #[test]
    fn a_definition_takes_the_name_and_type_of_its_declaration() {
        let mut firmware = Firmware::new();
        let int = firmware.base("int", gimli::DW_ATE_signed, 4);
        let root = firmware.root();
        let declaration = firmware.add(root, gimli::DW_TAG_variable, Some("limit"));
        firmware.set(declaration, gimli::DW_AT_type, Value::UnitRef(int));
        firmware.set(declaration, gimli::DW_AT_declaration, Value::Flag(true));
        let definition = firmware.add(root, gimli::DW_TAG_variable, None);
        firmware.set(
            definition,
            gimli::DW_AT_specification,
            Value::UnitRef(declaration),
        );
        let mut location = Expression::new();
        location.op_addr(Address::Constant(0x2000_0030));
        firmware.set(definition, gimli::DW_AT_location, Value::Exprloc(location));

        let found = variables(&firmware.elf(&[])).unwrap();
        // The declaration alone has no address: only the definition is there.
        assert_eq!(found.len(), 1);
        assert_eq!(find(&found, "limit").address, 0x2000_0030);
        assert_eq!(find(&found, "limit").kind, Kind::I32);
    }

    #[test]
    fn without_debug_information_the_symbols_are_read_by_size() {
        let elf = Firmware::new().elf(&[
            ("tick", 0x2000_0040, 4),
            ("mode", 0x2000_0044, 1),
            ("table", 0x2000_0048, 12),
        ]);
        let found = variables(&elf).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(find(&found, "tick").kind, Kind::U32);
        assert_eq!(find(&found, "tick").type_name, "4 bytes");
        assert_eq!(find(&found, "mode").kind, Kind::U8);
    }

    #[test]
    fn statics_of_one_name_are_named_after_their_source_files() {
        let mut firmware = Firmware::new();
        firmware.source("../Core/Src/uart.c");
        let uint = firmware.base("unsigned int", gimli::DW_ATE_unsigned, 4);
        let root = firmware.root();
        firmware.variable(root, "state", uint, 0x2000_0000);
        firmware.variable(root, "ticks", uint, 0x2000_0004);
        // Two of one name in one function, each in a block of its own.
        let poll = firmware.add(root, gimli::DW_TAG_subprogram, Some("poll"));
        for address in [0x2000_0008, 0x2000_000c] {
            let block = firmware.add(poll, gimli::DW_TAG_lexical_block, None);
            firmware.variable(block, "count", uint, address);
        }
        firmware.source(r"C:\work\Core\Src\spi.c");
        let uint = firmware.base("unsigned int", gimli::DW_ATE_unsigned, 4);
        let root = firmware.root();
        firmware.variable(root, "state", uint, 0x2000_0010);

        let found = variables(&firmware.elf(&[])).unwrap();
        assert_eq!(find(&found, "'uart.c'::state").address, 0x2000_0000);
        assert_eq!(find(&found, "'spi.c'::state").address, 0x2000_0010);
        // One of its name keeps it as it is, and two in one file are numbered.
        assert_eq!(find(&found, "ticks").address, 0x2000_0004);
        assert_eq!(find(&found, "poll::count").address, 0x2000_0008);
        assert_eq!(find(&found, "poll::count (2)").address, 0x2000_000c);
        assert_eq!(found.len(), 5);
    }

    #[test]
    fn without_debug_information_statics_are_named_after_their_files_too() {
        let mut elf = Writer::new(BinaryFormat::Elf, Architecture::Arm, Endianness::Little);
        let bss = elf.add_section(Vec::new(), b".bss".to_vec(), SectionKind::UninitializedData);
        elf.append_section_bss(bss, 0x2000_1000, 4);
        for (file, address) in [("uart.c", 0x2000_0000), ("spi.c", 0x2000_0004)] {
            elf.add_file_symbol(file.as_bytes().to_vec());
            elf.add_symbol(Symbol {
                name: b"state".to_vec(),
                value: address,
                size: 4,
                kind: SymbolKind::Data,
                scope: SymbolScope::Compilation,
                weak: false,
                section: SymbolSection::Section(bss),
                flags: SymbolFlags::None,
            });
        }

        let found = variables(&elf.write().unwrap()).unwrap();
        assert_eq!(find(&found, "'uart.c'::state").address, 0x2000_0000);
        assert_eq!(find(&found, "'spi.c'::state").address, 0x2000_0004);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn something_else_is_no_elf() {
        assert!(
            variables(b"not an elf at all")
                .unwrap_err()
                .starts_with("It is not an ELF file")
        );
    }

    #[test]
    fn bytes_are_read_as_their_kind() {
        assert_eq!(Kind::U16.decode(&[0x34, 0x12]), f64::from(0x1234));
        assert_eq!(Kind::I16.decode(&[0xff, 0xff]), -1.0);
        assert_eq!(Kind::I8.decode(&[0x80]), -128.0);
        assert_eq!(Kind::F32.decode(&21.5f32.to_le_bytes()), 21.5);
        assert_eq!(Kind::F64.decode(&(-0.125f64).to_le_bytes()), -0.125);
        assert_eq!(Kind::I64.decode(&(-5i64).to_le_bytes()), -5.0);
        assert_eq!(Kind::Bool.decode(&[2]), 1.0);
    }
}
