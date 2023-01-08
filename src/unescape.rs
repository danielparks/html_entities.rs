// See the normative reference for HTML5 entities:
// https://html.spec.whatwg.org/multipage/named-characters.html#named-character-references
//
// Entities do not always require a trailing semicolon, though the exact rules
// depend on whether the entity appears in an attribute value or somewhere else.
// See [`unescape_in()`] for more information.
//
// Some entities are prefixes for multiple other entities. For example:
//   &times &times; &timesb; &timesbar; &timesd;

use std::char;
use std::cmp::min;
use std::iter::Peekable;
use std::num::IntErrorKind;

// Include the ENTITIES map generated by build.rs
include!(concat!(env!("OUT_DIR"), "/entities.rs"));

/// The context for an input string (requires `unescape` feature).
///
/// Either `Attribute` for strings from an attribute value, or `General` for
/// everything else.
///
/// See [`unescape_in()`] for usage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Context {
    General,
    Attribute,
}

/// Expand all valid entities (requires `unescape` feature).
///
/// This is appropriate to use on any text outside of an attribute. See
/// [`unescape_in()`] for more information.
///
pub fn unescape<S: AsRef<[u8]>>(escaped: S) -> String {
    unescape_in(escaped, Context::General)
}

/// Expand all valid entities in an attribute (requires `unescape` feature).
///
/// This is only appropriate for the value of an attribute. See
/// [`unescape_in()`] for more information.
///
/// [specifies]: https://html.spec.whatwg.org/multipage/parsing.html#named-character-reference-state
pub fn unescape_attribute<S: AsRef<[u8]>>(escaped: S) -> String {
    unescape_in(escaped, Context::Attribute)
}

/// Expand all valid entities in a given context (requires `unescape` feature).
///
/// `context` may be:
///
///   * `Context::General`: use the rules for text outside of an attribute.
///      This is usually what you want.
///   * `Context::Attribute`: use the rules for attribute values.
///
/// This uses the [algorithm described] in the WHATWG spec. In attributes,
/// [named entities] without trailing semicolons are treated differently. They
/// not expanded if they are followed by an alphanumeric character or or `=`.
///
/// For example:
///
/// ```rust
/// use htmlize::*;
/// use assert2::check;
///
/// check!(unescape_in("&times",   Context::General)   == "×");
/// check!(unescape_in("&times",   Context::Attribute) == "×");
/// check!(unescape_in("&times;X", Context::General)   == "×X");
/// check!(unescape_in("&times;X", Context::Attribute) == "×X");
/// check!(unescape_in("&timesX",  Context::General)   == "×X");
/// check!(unescape_in("&timesX",  Context::Attribute) == "&timesX");
/// check!(unescape_in("&times=",  Context::General)   == "×=");
/// check!(unescape_in("&times=",  Context::Attribute) == "&times=");
/// check!(unescape_in("&times#",  Context::General)   == "×#");
/// check!(unescape_in("&times#",  Context::Attribute) == "×#");
/// ```
///
/// [algorithm described]: https://html.spec.whatwg.org/multipage/parsing.html#character-reference-state
/// [named entities]: https://html.spec.whatwg.org/multipage/parsing.html#named-character-reference-state
pub fn unescape_in<S: AsRef<[u8]>>(escaped: S, context: Context) -> String {
    let escaped = escaped.as_ref();
    let mut iter = escaped.iter().peekable();

    // Most (all?) entities are longer than their expansion, so allocating the
    // output buffer to be the same size as the input will usually prevent
    // multiple allocations and generally won’t over-allocate by very much.
    let mut buffer = Vec::with_capacity(escaped.len());

    while let Some(c) = iter.next() {
        if *c == b'&' {
            let mut expansion = match_entity(&mut iter, context);
            buffer.append(&mut expansion);
        } else {
            buffer.push(*c);
        }
    }

    String::from_utf8(buffer).unwrap()
}

const PEEK_MATCH_ERROR: &str = "iter.next() did not match previous iter.peek()";

#[allow(clippy::from_str_radix_10)]
fn match_numeric_entity<'a, I>(iter: &mut Peekable<I>) -> Vec<u8>
where
    I: Iterator<Item = &'a u8>,
{
    let c = iter.next().expect(PEEK_MATCH_ERROR);
    if *c != b'#' {
        panic!("{}", PEEK_MATCH_ERROR);
    }

    let mut best_expansion = vec![b'&', b'#'];

    let number = match iter.peek() {
        Some(&b'x') | Some(&b'X') => {
            // Hexadecimal entity
            best_expansion.push(*iter.next().expect(PEEK_MATCH_ERROR));

            let hex = consume_hexadecimal(iter);
            best_expansion.extend_from_slice(&hex);

            u32::from_str_radix(&String::from_utf8(hex).unwrap(), 16)
        }
        Some(_) => {
            // Presumably a decimal entity
            let dec = consume_decimal(iter);
            best_expansion.extend_from_slice(&dec);

            u32::from_str_radix(&String::from_utf8(dec).unwrap(), 10)
        }
        None => {
            // Iterator reached end
            return best_expansion;
        }
    };

    if let Some(&b';') = iter.peek() {
        best_expansion.push(*iter.next().expect(PEEK_MATCH_ERROR));
    } else {
        // missing-semicolon-after-character-reference: ignore and continue.
        // https://html.spec.whatwg.org/multipage/parsing.html#parse-error-missing-semicolon-after-character-reference
    }

    match number {
        Ok(number) => {
            if let Some(expansion) = correct_numeric_entity(number) {
                return expansion;
            }
        }
        Err(error) => match error.kind() {
            IntErrorKind::PosOverflow => {
                // Too large a number
                return char_to_vecu8(REPLACEMENT_CHAR).unwrap();
            }
            IntErrorKind::Empty => {
                // No number, e.g. &#; or &#x;. Fall through.
            }
            _ => panic!("error parsing number in numeric entity: {:?}", error),
        },
    }

    best_expansion
}

/// Unicode replacement character (U+FFFD “�”, requires `unescape` feature)
///
/// According to the WHATWG HTML spec, this is used as an expansion for certain
/// invalid numeric entities.
///
/// According to Unicode 12, this is “used to replace an incoming character
/// whose value is unknown or unrepresentable in Unicode.” The latest chart for
/// the Specials block is [available as a PDF](https://www.unicode.org/charts/PDF/UFFF0.pdf).
pub const REPLACEMENT_CHAR: char = '\u{fffd}';

// https://html.spec.whatwg.org/multipage/parsing.html#parse-error-character-reference-outside-unicode-range
fn is_outside_range<C: Into<u32>>(c: C) -> bool {
    c.into() > 0x10FFFF
}

// https://infra.spec.whatwg.org/#surrogate
fn is_surrogate<C: Into<u32>>(c: C) -> bool {
    (0xD800..=0xDFFF).contains(&c.into())
}

#[inline]
fn char_to_vecu8(c: char) -> Option<Vec<u8>> {
    Some(c.to_string().into())
}

#[inline]
fn u32_to_vecu8(c: u32) -> Option<Vec<u8>> {
    Some(char::from_u32(c).unwrap().to_string().into())
}

// https://html.spec.whatwg.org/multipage/parsing.html#numeric-character-reference-end-state
fn correct_numeric_entity(number: u32) -> Option<Vec<u8>> {
    match number {
        // null-character-reference parse error:
        0x00 => char_to_vecu8(REPLACEMENT_CHAR),

        // character-reference-outside-unicode-range parse error:
        c if is_outside_range(c) => char_to_vecu8(REPLACEMENT_CHAR),

        // surrogate-character-reference parse error:
        c if is_surrogate(c) => char_to_vecu8(REPLACEMENT_CHAR),

        // control-character-reference parse error exceptions:
        0x80 => u32_to_vecu8(0x20AC), // EURO SIGN (€)
        0x82 => u32_to_vecu8(0x201A), // SINGLE LOW-9 QUOTATION MARK (‚)
        0x83 => u32_to_vecu8(0x0192), // LATIN SMALL LETTER F WITH HOOK (ƒ)
        0x84 => u32_to_vecu8(0x201E), // DOUBLE LOW-9 QUOTATION MARK („)
        0x85 => u32_to_vecu8(0x2026), // HORIZONTAL ELLIPSIS (…)
        0x86 => u32_to_vecu8(0x2020), // DAGGER (†)
        0x87 => u32_to_vecu8(0x2021), // DOUBLE DAGGER (‡)
        0x88 => u32_to_vecu8(0x02C6), // MODIFIER LETTER CIRCUMFLEX ACCENT (ˆ)
        0x89 => u32_to_vecu8(0x2030), // PER MILLE SIGN (‰)
        0x8A => u32_to_vecu8(0x0160), // LATIN CAPITAL LETTER S WITH CARON (Š)
        0x8B => u32_to_vecu8(0x2039), // SINGLE LEFT-POINTING ANGLE QUOTATION MARK (‹)
        0x8C => u32_to_vecu8(0x0152), // LATIN CAPITAL LIGATURE OE (Œ)
        0x8E => u32_to_vecu8(0x017D), // LATIN CAPITAL LETTER Z WITH CARON (Ž)
        0x91 => u32_to_vecu8(0x2018), // LEFT SINGLE QUOTATION MARK (‘)
        0x92 => u32_to_vecu8(0x2019), // RIGHT SINGLE QUOTATION MARK (’)
        0x93 => u32_to_vecu8(0x201C), // LEFT DOUBLE QUOTATION MARK (“)
        0x94 => u32_to_vecu8(0x201D), // RIGHT DOUBLE QUOTATION MARK (”)
        0x95 => u32_to_vecu8(0x2022), // BULLET (•)
        0x96 => u32_to_vecu8(0x2013), // EN DASH (–)
        0x97 => u32_to_vecu8(0x2014), // EM DASH (—)
        0x98 => u32_to_vecu8(0x02DC), // SMALL TILDE (˜)
        0x99 => u32_to_vecu8(0x2122), // TRADE MARK SIGN (™)
        0x9A => u32_to_vecu8(0x0161), // LATIN SMALL LETTER S WITH CARON (š)
        0x9B => u32_to_vecu8(0x203A), // SINGLE RIGHT-POINTING ANGLE QUOTATION MARK (›)
        0x9C => u32_to_vecu8(0x0153), // LATIN SMALL LIGATURE OE (œ)
        0x9E => u32_to_vecu8(0x017E), // LATIN SMALL LETTER Z WITH CARON (ž)
        0x9F => u32_to_vecu8(0x0178), // LATIN CAPITAL LETTER Y WITH DIAERESIS (Ÿ)

        // A few parse errors and other cases are handled by the catch-all.
        //
        //   * noncharacter-character-reference parse error
        //   * control-character-reference parse error
        //   * 0x0d (carriage return)
        //   * ASCII whitespace
        //   * ASCII control characters
        //
        // I found the spec a little confusing here, but a close reading and
        // some browser testing convinced me that all of these cases are handled
        // but just emitting the represented code point.

        // Everything else.
        c => match char::from_u32(c) {
            Some(c) => char_to_vecu8(c),
            None => None,
        },
    }
}

macro_rules! consumer {
    ($name:ident, $($accept:pat)|+) => {
        fn $name<'a, I>(iter: &mut Peekable<I>) -> Vec<u8>
            where I: Iterator<Item = &'a u8>
        {
            let mut buffer: Vec<u8> = Vec::new();
            while let Some(c) = iter.peek() {
                match **c {
                    $($accept)|+ => {
                        buffer.push(*iter.next().expect(PEEK_MATCH_ERROR));
                    },
                    _ => { return buffer; },
                }
            }

            return buffer;
        }
    }
}

consumer!(consume_decimal, b'0'..=b'9');
consumer!(consume_hexadecimal, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F');
consumer!(consume_alphanumeric, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z');

fn match_entity<'a, I>(iter: &mut Peekable<I>, context: Context) -> Vec<u8>
where
    I: Iterator<Item = &'a u8>,
{
    if let Some(&b'#') = iter.peek() {
        // Numeric entity.
        return match_numeric_entity(iter);
    }

    // Determine longest possible candidate including & and any trailing ;.
    let mut candidate = vec![b'&'];
    candidate.append(&mut consume_alphanumeric(iter));

    match iter.peek() {
        Some(&b';') => {
            // Actually consume the semicolon.
            candidate.push(*iter.next().expect(PEEK_MATCH_ERROR));
        }
        Some(b'=') if context == Context::Attribute => {
            // Special case, see https://html.spec.whatwg.org/multipage/parsing.html#named-character-reference-state
            // This character cannot be alphanumeric, since all alphanumeric
            // characters were consumed above.
            return candidate;
        }
        _ => {
            // missing-semicolon-after-character-reference: ignore and continue.
            // https://html.spec.whatwg.org/multipage/parsing.html#parse-error-missing-semicolon-after-character-reference
        }
    }

    if candidate.len() < ENTITY_MIN_LENGTH {
        // Couldn’t possibly match.
        return candidate;
    }

    if context == Context::Attribute {
        // If candidate does not exactly match an entity, then don't expand it.
        // This is because of the special case described in the spec (see
        // https://html.spec.whatwg.org/multipage/parsing.html#named-character-reference-state)
        // Essentially it says that *in attributes* entities must be terminated
        // with a semicolon, EOF, or some character *other* than [a-zA-Z0-9=].
        //
        // In other words, “&timesa” expands to “&timesa” in an attribute rather
        // than “×a”.
        if let Some(expansion) = ENTITIES.get(&candidate) {
            return expansion.to_vec();
        }
    } else {
        // Find longest matching entity.
        let max_len = min(candidate.len(), ENTITY_MAX_LENGTH);
        for check_len in (ENTITY_MIN_LENGTH..=max_len).rev() {
            if let Some(expansion) = ENTITIES.get(&candidate[..check_len]) {
                // Found a match.
                let mut result = Vec::with_capacity(
                    expansion.len() + candidate.len() - check_len,
                );
                result.extend_from_slice(expansion);

                if check_len < candidate.len() {
                    // Need to append the rest of the consumed bytes.
                    result.extend_from_slice(&candidate[check_len..]);
                }

                return result;
            }
        }
    }

    // Did not find a match.
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use paste::paste;

    // Test both unescape and unescape_attribute
    macro_rules! test_both {
        ($name:ident, unescape $($test:tt)+) => {
            #[test]
            fn $name() {
                ::assert2::assert!(unescape$($test)+);
            }

            paste! {
                #[test]
                fn [<attribute_ $name>]() {
                    ::assert2::assert!(unescape_attribute$($test)+);
                }
            }
        };
    }

    test_both!(almost_entity, unescape("&time") == "&time");
    test_both!(exact_times, unescape("&times;") == "×");
    test_both!(exact_timesb, unescape("&timesb;") == "⊠");
    test_both!(bare_times_end, unescape("&times") == "×");
    test_both!(bare_times_bang, unescape("&times!") == "×!");

    test!(bare_entity_char, unescape("&timesa") == "×a");
    test!(bare_entity_equal, unescape("&times=") == "×=");
    test!(bare_entity_char_is_prefix, unescape("&timesb") == "×b");
    test!(
        attribute_bare_entity_char,
        unescape_attribute("&timesa") == "&timesa"
    );
    test!(
        attribute_bare_entity_equal,
        unescape_attribute("&times=") == "&times="
    );
    test!(
        attribute_bare_entity_char_is_prefix,
        unescape_attribute("&timesb") == "&timesb"
    );

    test_both!(empty, unescape("") == "");
    test_both!(no_entities, unescape("none") == "none");
    test_both!(only_ampersand, unescape("&") == "&");
    test_both!(empty_entity, unescape("&;") == "&;");
    test_both!(middle_entity, unescape(" &amp; ") == " & ");
    test_both!(extra_ampersands, unescape("&&amp;&") == "&&&");
    test_both!(two_entities, unescape("AND &amp;&AMP; and") == "AND && and");
    test_both!(
        long_entity,
        unescape("&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;")
            == "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;"
    );

    test_both!(correct_hex_lowerx_lower, unescape("&#x7a;") == "z");
    test_both!(correct_hex_lowerx_upper, unescape("&#x7A;") == "z");
    test_both!(correct_hex_upperx_lower, unescape("&#X7a;") == "z");
    test_both!(correct_hex_upperx_upper, unescape("&#X7A;") == "z");
    test_both!(correct_hex_leading_zero, unescape("&#x07a;") == "z");
    test_both!(correct_hex_leading_zero_zero, unescape("&#x007a;") == "z");
    test_both!(correct_dec, unescape("&#122;") == "z");
    test_both!(correct_dec_leading_zero, unescape("&#0122;") == "z");
    test_both!(correct_dec_leading_zero_zero, unescape("&#00122;") == "z");
    test_both!(correct_hex_unicode, unescape("&#x21D2;") == "⇒");

    test_both!(bare_hex_char, unescape("&#x7Az") == "zz");
    test_both!(bare_hex_end, unescape("&#x7A") == "z");
    test_both!(bare_dec_char, unescape("&#122z") == "zz");
    test_both!(bare_dec_end, unescape("&#122") == "z");

    test_both!(hex_instead_of_dec, unescape("&#a0;") == "&#a0;");
    test_both!(invalid_hex_lowerx, unescape("&#xZ;") == "&#xZ;");
    test_both!(invalid_hex_upperx, unescape("&#XZ;") == "&#XZ;");

    test_both!(hex_control_1, unescape("&#x1;") == "\u{1}");
    test_both!(dec_control_1, unescape("&#1;") == "\u{1}");
    test_both!(dec_cr, unescape("&#13;") == "\r");
    test_both!(hex_cr, unescape("&#xd;") == "\r");
    test_both!(hex_tab, unescape("&#9;") == "\t");
    test_both!(dec_tab, unescape("&#9;") == "\t");

    test_both!(hex_max_code_point, unescape("&#x10ffff;") == "\u{10ffff}");
    test_both!(
        hex_above_max_code_point,
        unescape("&#x110001;") == "\u{fffd}"
    );
    test_both!(hex_11_chars, unescape("&#x1100000000;") == "\u{fffd}");
    test_both!(
        bare_hex_11_chars_end,
        unescape("&#x1100000000") == "\u{fffd}"
    );

    test_both!(
        hex_40_chars,
        unescape("&#x110000000000000000000000000000000000000;") == "\u{fffd}"
    );
    test_both!(
        bare_hex_40_chars_end,
        unescape("&#x110000000000000000000000000000000000000") == "\u{fffd}"
    );

    test_both!(special_entity_null, unescape("&#0;") == "\u{fffd}");
    test_both!(special_entity_bullet, unescape("&#x95;") == "•");
    test_both!(
        special_entity_bullets,
        unescape("&#x95;&#149;&#x2022;•") == "••••"
    );
    test_both!(special_entity_space, unescape("&#x20") == " ");

    const ALL_SOURCE: &str =
        include_str!("../tests/corpus/all-entities-source.txt");
    const ALL_EXPANDED: &str =
        include_str!("../tests/corpus/all-entities-expanded.txt");
    test_both!(all_entities, unescape(ALL_SOURCE) == ALL_EXPANDED);
}
