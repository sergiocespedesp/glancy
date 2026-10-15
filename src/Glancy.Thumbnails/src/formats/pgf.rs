//! Progressive Graphics File (`.pgf`, libPGF): not supported.
//!
//! PGF stores a wavelet transform encoded with libPGF's own scheme (bit planes with adaptive run-length codes
//! per macroblock). There is no published specification apart from the libPGF source code, nor a pure-Rust
//! implementation, and without reference files a reimplementation cannot be verified; nor does the format carry
//! an embedded preview. Rather than show an incorrect thumbnail, the file is recognized by its signature and a
//! clear error is returned.

use super::{Content, Ctx};
use crate::bail;
use crate::error::Result;

pub fn render(ctx: &Ctx) -> Result<Content> {
    if !ctx.src.read_vec(0, 3)?.starts_with(b"PGF") {
        bail!("Not a PGF file");
    }
    bail!("Unsupported PGF")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    fn run(data: &[u8]) -> String {
        let settings = FormatSettings::defaults(FormatId::get("pgf").unwrap());
        let src = MemorySource(data.to_vec());
        render(&Ctx { src: &src, path: None, extension: ".pgf", size: 256, settings: &settings }).err().unwrap().0
    }

    #[test]
    fn clear_errors() {
        assert_eq!(run(b"PGF6\x10\0\0\0"), "Unsupported PGF");
        assert_eq!(run(b"xx"), "Not a PGF file");
    }
}
