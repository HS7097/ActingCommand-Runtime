// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0c: the import table of a PE file (a DLL), read from its bytes, for
//! `--probe-dlls`: which modules a DLL that failed to load imports, and which functions of each.
//! Only the regular import directory is read (delay-load imports cannot fail a load).

/// The most import descriptors, functions per module and name bytes the reader accepts.
const MAX_MODULES: usize = 4_096;
const MAX_FUNCTIONS: usize = 65_536;
const MAX_NAME_BYTES: usize = 4_096;

/// `IMAGE_NT_OPTIONAL_HDR32_MAGIC` and `IMAGE_NT_OPTIONAL_HDR64_MAGIC`.
const PE32_MAGIC: u16 = 0x10b;
const PE32_PLUS_MAGIC: u16 = 0x20b;
/// `IMAGE_DIRECTORY_ENTRY_IMPORT`.
const IMPORT_DIRECTORY: usize = 1;

/// One module the import table names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedModule {
    pub name: String,
    pub functions: Vec<ImportedFunction>,
}

/// One imported function: by name, or by ordinal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportedFunction {
    Name(String),
    Ordinal(u16),
}

impl ImportedFunction {
    pub fn text(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Ordinal(ordinal) => format!("#{ordinal}"),
        }
    }
}

struct Image<'a> {
    bytes: &'a [u8],
    pe32_plus: bool,
    /// (virtual address, virtual size, raw size, raw offset) per section.
    sections: Vec<(u32, u32, u32, u32)>,
}

impl Image<'_> {
    fn u16_at(&self, offset: usize) -> Result<u16, String> {
        self.bytes
            .get(offset..offset + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| format!("the file ends before offset {offset:#x}"))
    }

    fn u32_at(&self, offset: usize) -> Result<u32, String> {
        self.bytes
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| format!("the file ends before offset {offset:#x}"))
    }

    fn u64_at(&self, offset: usize) -> Result<u64, String> {
        self.bytes
            .get(offset..offset + 8)
            .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
            .ok_or_else(|| format!("the file ends before offset {offset:#x}"))
    }

    /// The file offset of a relative virtual address.
    fn offset_of(&self, rva: u32) -> Result<usize, String> {
        for &(address, virtual_size, raw_size, raw_offset) in &self.sections {
            let extent = virtual_size.max(raw_size);
            if rva >= address && rva - address < extent {
                let offset = (rva - address) as usize + raw_offset as usize;
                if offset >= self.bytes.len() {
                    break;
                }
                return Ok(offset);
            }
        }
        Err(format!("RVA {rva:#x} lies in no section of the file"))
    }

    fn c_string(&self, rva: u32) -> Result<String, String> {
        let start = self.offset_of(rva)?;
        let tail = &self.bytes[start..];
        let length = tail
            .iter()
            .take(MAX_NAME_BYTES)
            .position(|byte| *byte == 0)
            .ok_or_else(|| format!("the name at RVA {rva:#x} is unterminated or too long"))?;
        Ok(String::from_utf8_lossy(&tail[..length]).into_owned())
    }
}

/// The modules and functions the regular import table of `bytes` names, in table order.
pub fn imports(bytes: &[u8]) -> Result<Vec<ImportedModule>, String> {
    let mut image = Image {
        bytes,
        pe32_plus: true,
        sections: Vec::new(),
    };
    if bytes.get(0..2) != Some(b"MZ".as_slice()) {
        return Err("no MZ header".to_owned());
    }
    let nt = image.u32_at(0x3c)? as usize;
    if bytes.get(nt..nt + 4) != Some(b"PE\0\0".as_slice()) {
        return Err(format!("no PE signature at {nt:#x}"));
    }
    let coff = nt + 4;
    let section_count = image.u16_at(coff + 2)? as usize;
    let optional_size = image.u16_at(coff + 16)? as usize;
    let optional = coff + 20;
    let (directory_count_at, directories) = match image.u16_at(optional)? {
        PE32_PLUS_MAGIC => (optional + 108, optional + 112),
        PE32_MAGIC => {
            image.pe32_plus = false;
            (optional + 92, optional + 96)
        }
        other => return Err(format!("unknown optional header magic {other:#x}")),
    };
    let section_table = optional + optional_size;
    for index in 0..section_count {
        let entry = section_table + index * 40;
        image.sections.push((
            image.u32_at(entry + 12)?,
            image.u32_at(entry + 8)?,
            image.u32_at(entry + 16)?,
            image.u32_at(entry + 20)?,
        ));
    }
    if (image.u32_at(directory_count_at)? as usize) <= IMPORT_DIRECTORY {
        return Ok(Vec::new());
    }
    let import_rva = image.u32_at(directories + IMPORT_DIRECTORY * 8)?;
    if import_rva == 0 {
        return Ok(Vec::new());
    }
    let table = image.offset_of(import_rva)?;
    let mut modules = Vec::new();
    for index in 0..=MAX_MODULES {
        if index == MAX_MODULES {
            return Err(format!(
                "the import table names more than {MAX_MODULES} modules"
            ));
        }
        let descriptor = table + index * 20;
        let original_first_thunk = image.u32_at(descriptor)?;
        let name_rva = image.u32_at(descriptor + 12)?;
        let first_thunk = image.u32_at(descriptor + 16)?;
        if name_rva == 0 && original_first_thunk == 0 && first_thunk == 0 {
            break;
        }
        let name = image.c_string(name_rva)?;
        let thunks = if original_first_thunk != 0 {
            original_first_thunk
        } else {
            first_thunk
        };
        let functions = functions(&image, thunks)
            .map_err(|error| format!("the imports from {name}: {error}"))?;
        modules.push(ImportedModule { name, functions });
    }
    Ok(modules)
}

fn functions(image: &Image<'_>, thunks: u32) -> Result<Vec<ImportedFunction>, String> {
    let mut functions = Vec::new();
    if thunks == 0 {
        return Ok(functions);
    }
    let start = image.offset_of(thunks)?;
    let width = if image.pe32_plus { 8 } else { 4 };
    for index in 0..=MAX_FUNCTIONS {
        if index == MAX_FUNCTIONS {
            return Err(format!("more than {MAX_FUNCTIONS} functions"));
        }
        let at = start + index * width;
        let (entry, by_ordinal) = if image.pe32_plus {
            let entry = image.u64_at(at)?;
            (entry, entry & (1 << 63) != 0)
        } else {
            let entry = u64::from(image.u32_at(at)?);
            (entry, entry & (1 << 31) != 0)
        };
        if entry == 0 {
            break;
        }
        if by_ordinal {
            functions.push(ImportedFunction::Ordinal((entry & 0xffff) as u16));
        } else {
            let hint_name = u32::try_from(entry & 0x7fff_ffff)
                .map_err(|_| format!("the hint/name RVA {entry:#x} is out of range"))?;
            functions.push(ImportedFunction::Name(
                image.c_string(hint_name.saturating_add(2))?,
            ));
        }
    }
    Ok(functions)
}
