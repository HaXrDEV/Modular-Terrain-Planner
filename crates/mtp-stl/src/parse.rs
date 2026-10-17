//! Reading binary and ASCII STL into a triangle soup.
//!
//! Stored facet normals are ignored: many exporters write zeros, and the
//! renderer computes its own from the corners anyway.

use thiserror::Error;

/// Three corners in file units, in the order the file lists them.
pub type Triangle = [[f32; 3]; 3];

const HEADER_LEN: usize = 80;
const BINARY_PREFIX: usize = HEADER_LEN + 4;
const RECORD_LEN: usize = 50;
/// How much of the start of a file is looked at to tell ASCII from binary.
const SNIFF_LEN: usize = 512;
/// A binary header claiming more triangles than this is taken to be some other
/// kind of file rather than a truncated STL. Real tiles stay far below it.
const PLAUSIBLE_TRIANGLES: u64 = 50_000_000;

/// Why a file couldn't be read as STL. The messages are shown to users.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum StlError {
    #[error("the file is empty")]
    Empty,
    #[error("the file is cut short: it says {expected} triangles but holds {found}")]
    Truncated { expected: u32, found: u32 },
    #[error("this is not an STL file")]
    NotStl,
    #[error("the file holds no triangles")]
    NoTriangles,
    #[error("line {line} can't be read: {message}")]
    Ascii { line: usize, message: String },
    #[error("the file holds coordinates that aren't numbers (NaN or infinity)")]
    NonFinite,
}

/// Parses a whole STL file, binary or ASCII.
pub fn parse_stl(bytes: &[u8]) -> Result<Vec<Triangle>, StlError> {
    if bytes.is_empty() {
        return Err(StlError::Empty);
    }
    let claimed = claimed_triangles(bytes);
    let solid = starts_with_solid(bytes);

    // Text bytes read as a count are always above PLAUSIBLE_TRIANGLES, so an
    // ASCII file can't be mistaken for a truncated binary one.
    let triangles = if solid && looks_like_ascii(bytes) {
        parse_ascii(bytes)?
    } else if let Some(n) = claimed.filter(|&n| bytes.len() as u64 >= binary_len(n)) {
        parse_binary(bytes, n)?
    } else if let Some(n) = claimed.filter(|&n| n > 0 && u64::from(n) <= PLAUSIBLE_TRIANGLES) {
        let found = (bytes.len() - BINARY_PREFIX) / RECORD_LEN;
        return Err(StlError::Truncated { expected: n, found: u32::try_from(found).unwrap_or(u32::MAX) });
    } else if solid {
        parse_ascii(bytes)?
    } else {
        return Err(StlError::NotStl);
    };

    if triangles.is_empty() { Err(StlError::NoTriangles) } else { Ok(triangles) }
}

/// The triangle count of a binary header, if the file is long enough to have one.
fn claimed_triangles(bytes: &[u8]) -> Option<u32> {
    let count = bytes.get(HEADER_LEN..BINARY_PREFIX)?;
    Some(u32::from_le_bytes(count.try_into().ok()?))
}

fn binary_len(n: u32) -> u64 {
    BINARY_PREFIX as u64 + RECORD_LEN as u64 * u64::from(n)
}

fn starts_with_solid(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    bytes[start..].get(..5).is_some_and(|w| w.eq_ignore_ascii_case(b"solid"))
}

/// A `solid` header on a binary file of the right size is common, so the
/// start of the file also has to read as text with a facet in it.
fn looks_like_ascii(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    let text = head.iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace());
    text && head.windows(5).any(|w| w.eq_ignore_ascii_case(b"facet"))
}

fn parse_binary(bytes: &[u8], n: u32) -> Result<Vec<Triangle>, StlError> {
    let n = n as usize;
    let body = &bytes[BINARY_PREFIX..BINARY_PREFIX + n * RECORD_LEN];
    let mut triangles = Vec::with_capacity(n);
    for record in body.as_chunks::<RECORD_LEN>().0 {
        // 12 bytes of normal, then three corners, then 2 attribute bytes
        let mut tri = [[0.0f32; 3]; 3];
        for (i, chunk) in record[12..48].as_chunks::<4>().0.iter().enumerate() {
            let value = f32::from_le_bytes(*chunk);
            if !value.is_finite() {
                return Err(StlError::NonFinite);
            }
            tri[i / 3][i % 3] = value;
        }
        triangles.push(tri);
    }
    Ok(triangles)
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Outside,
    Solid,
    Facet,
    Loop(usize),
    EndLoop,
}

fn parse_ascii(bytes: &[u8]) -> Result<Vec<Triangle>, StlError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    // Words with the line they're on. Lines end in LF, CRLF or a lone CR
    // (old Mac files), and exporters put keywords on lines as they like, so
    // the grammar is read word by word. The rest of a "solid" or
    // "endsolid" line is the solid's name, which may be in any encoding.
    let text = String::from_utf8_lossy(bytes);
    let mut words: Vec<(usize, &str)> = Vec::new();
    for (index, raw) in text.split(['\n', '\r']).enumerate() {
        for word in raw.split_ascii_whitespace() {
            words.push((index + 1, word));
            if word.eq_ignore_ascii_case("solid") || word.eq_ignore_ascii_case("endsolid") {
                break;
            }
        }
    }
    // A lone CR followed by LF counts as one line break for line numbers
    let crlf = text.contains("\r\n");
    let line_of = |raw: usize| if crlf { raw.div_ceil(2) } else { raw };

    let mut triangles = Vec::new();
    let mut state = State::Outside;
    let mut tri = [[0.0f32; 3]; 3];
    let mut last_line = 0;
    let mut i = 0;
    while i < words.len() {
        let (raw_line, word) = words[i];
        let line = line_of(raw_line);
        last_line = line;
        i += 1;
        let fail = |message: &str| StlError::Ascii { line, message: message.to_owned() };
        let is = |w: &str| word.eq_ignore_ascii_case(w);

        state = match state {
            State::Outside if is("solid") => State::Solid,
            State::Outside => return Err(fail("expected \"solid\"")),
            State::Solid if is("facet") => State::Facet,
            State::Solid if is("endsolid") => State::Outside,
            State::Solid => return Err(fail("expected \"facet\" or \"endsolid\"")),
            // "normal" and its three numbers, which nothing trusts anyway
            State::Facet if is("outer") => {
                if words.get(i).is_some_and(|(_, w)| w.eq_ignore_ascii_case("loop")) {
                    i += 1;
                }
                State::Loop(0)
            }
            State::Facet if is("normal") || word.parse::<f32>().is_ok() => State::Facet,
            State::Facet => return Err(fail("expected \"outer loop\"")),
            State::Loop(k) if k < 3 && is("vertex") => {
                let mut corner = [0.0f32; 3];
                for value in &mut corner {
                    let Some(&(_, w)) = words.get(i).filter(|(l, _)| line_of(*l) == line) else {
                        return Err(fail("a vertex needs three numbers"));
                    };
                    *value = w.parse().map_err(|_| fail(&format!("\"{w}\" is not a number")))?;
                    if !value.is_finite() {
                        return Err(StlError::NonFinite);
                    }
                    i += 1;
                }
                tri[k] = corner;
                State::Loop(k + 1)
            }
            State::Loop(k) if k < 3 => return Err(fail("expected \"vertex\"")),
            State::Loop(_) if is("endloop") => State::EndLoop,
            State::Loop(_) => return Err(fail("a facet must have exactly three vertices")),
            State::EndLoop if is("endfacet") => {
                triangles.push(tri);
                State::Solid
            }
            State::EndLoop => return Err(fail("expected \"endfacet\"")),
        };
    }

    // A missing "endsolid" is harmless; ending inside a facet is not
    match state {
        State::Outside | State::Solid => Ok(triangles),
        _ => Err(StlError::Ascii { line: last_line, message: "the file ends in the middle of a facet".to_owned() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary(header: &[u8], tris: &[Triangle]) -> Vec<u8> {
        let mut out = vec![b' '; HEADER_LEN];
        out[..header.len()].copy_from_slice(header);
        out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
        for tri in tris {
            out.extend_from_slice(&[0u8; 12]);
            for v in tri.iter().flatten() {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&[0u8; 2]);
        }
        out
    }

    const TRI: Triangle = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    #[test]
    fn binary_with_solid_header_is_binary() {
        let data = binary(b"solid facet but binary", &[TRI, TRI]);
        assert_eq!(parse_stl(&data), Ok(vec![TRI, TRI]));
    }

    #[test]
    fn padded_binary_is_accepted() {
        let mut data = binary(b"exporter", &[TRI]);
        data.extend_from_slice(&[0u8; 7]);
        assert_eq!(parse_stl(&data), Ok(vec![TRI]));
    }

    #[test]
    fn non_finite_is_rejected() {
        let data = binary(b"x", &[[[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]]);
        assert_eq!(parse_stl(&data), Err(StlError::NonFinite));
    }

    #[test]
    fn ascii_round_trip() {
        let text = "  solid t\nfacet normal 0 0 0\n outer loop\n  vertex 0 0 0\n  vertex 1 0 0\n  vertex 0 1 0\n \
                    endloop\nendfacet\nendsolid t\n";
        assert_eq!(parse_stl(text.as_bytes()), Ok(vec![TRI]));
    }

    #[test]
    fn ascii_errors_name_the_line() {
        let text = "solid t\nfacet normal 0 0 0\nouter loop\nvertex 0 0 x\n";
        let err = parse_stl(text.as_bytes()).expect_err("bad number");
        assert_eq!(err, StlError::Ascii { line: 4, message: "\"x\" is not a number".to_owned() });
        assert_eq!(err.to_string(), "line 4 can't be read: \"x\" is not a number");
    }

    #[test]
    fn ascii_with_old_mac_line_endings_or_one_line_facets() {
        let mac = "solid t\rfacet normal 0 0 0\router loop\rvertex 0 0 0\rvertex 1 0 0\rvertex 0 1 0\rendloop\rendfacet\rendsolid t\r";
        assert_eq!(parse_stl(mac.as_bytes()), Ok(vec![TRI]));
        let compact = "solid t\nfacet normal 0 0 1 outer loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop endfacet\nendsolid\n";
        assert_eq!(parse_stl(compact.as_bytes()), Ok(vec![TRI]));
        let crlf = "solid t\r\nfacet normal 0 0 0\r\nouter loop\r\nvertex 0 0 x\r\n";
        assert_eq!(
            parse_stl(crlf.as_bytes()),
            Err(StlError::Ascii { line: 4, message: "\"x\" is not a number".to_owned() })
        );
    }

    #[test]
    fn truncated_message() {
        let err = StlError::Truncated { expected: 100, found: 3 };
        assert_eq!(err.to_string(), "the file is cut short: it says 100 triangles but holds 3");
    }
}
