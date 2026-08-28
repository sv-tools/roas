//! A JSON number as it was written, rather than as `f64` remembers it.
//!
//! `serde_json` is built here with `arbitrary_precision`, so a parsed
//! number keeps its literal — `Number::as_str` hands back the very
//! characters the document carried. That is the one piece of evidence
//! floating point destroys, and having it turns every numeric question
//! this crate asks from a judgement call into arithmetic:
//!
//! - `1.0` **is** an integer and `1.0000000000000001` is not, where a
//!   double makes both of them `1.0`.
//! - `9007199254740993` sits above `maximum: 9007199254740992`, where a
//!   double makes the two equal.
//! - `0.3` **is** a multiple of `0.1`, where dividing the doubles gives
//!   `2.9999999999999996` and proves nothing.
//!
//! Every number is carried as `mantissa × 10^scale` with both parts
//! exact, and every comparison is integer arithmetic on those. What
//! remains uncertain is only what will not fit: a literal past `i128`'s
//! range, which is reported rather than approximated.

use std::cmp::Ordering;
use std::fmt::{self, Display, Formatter};

/// The most digits a mantissa can carry before `i128` gives out.
const MAX_DIGITS: u32 = 38;

/// A JSON number, exactly: `mantissa × 10^scale`.
///
/// Always normalized, so the same value has one representation —
/// `1.0`, `1`, `10e-1` and `0.01e2` are all `Decimal { 1, 0 }`, and
/// comparing them is comparing two pairs of integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Decimal {
    mantissa: i128,
    scale: i32,
}

impl Decimal {
    /// Parse a number as JSON writes one, plus the two spellings a
    /// *parameter* may arrive with that JSON forbids: a leading `+`,
    /// and leading zeros. A parameter is text on a wire, not a JSON
    /// document, and `?limit=010` is a client saying ten.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };

        let (significand, exponent) = match rest.find(['e', 'E']) {
            Some(at) => (&rest[..at], rest[at + 1..].parse::<i32>().ok()?),
            None => (rest, 0),
        };

        let (whole, fraction) = match significand.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (significand, ""),
        };
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        if !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
        {
            return None;
        }

        // The digits are read as one integer and the decimal point
        // becomes part of the scale, which is what makes `1.5` and
        // `15e-1` the same number here.
        //
        // A negative number accumulates downwards rather than being
        // built positive and negated: `i128::MIN`'s magnitude is one
        // larger than `i128::MAX`, so negating it at the end would
        // refuse the very number it is trying to read.
        let mut mantissa: i128 = 0;
        for digit in whole.bytes().chain(fraction.bytes()) {
            let digit = i128::from(digit - b'0');
            mantissa = mantissa.checked_mul(10)?;
            mantissa = if negative {
                mantissa.checked_sub(digit)?
            } else {
                mantissa.checked_add(digit)?
            };
        }
        let scale = exponent.checked_sub(i32::try_from(fraction.len()).ok()?)?;

        Self::new(mantissa, scale)
    }

    /// Normalized on the way in: trailing zeros move into the scale, so
    /// equal values are equal structs.
    ///
    /// `None` when normalizing would run the scale past `i32` — as
    /// `10e2147483647` does, a literal JSON accepts and nothing here can
    /// hold.
    fn new(mut mantissa: i128, mut scale: i32) -> Option<Self> {
        if mantissa == 0 {
            return Some(Self {
                mantissa: 0,
                scale: 0,
            });
        }
        while mantissa % 10 == 0 {
            mantissa /= 10;
            scale = scale.checked_add(1)?;
        }
        Some(Self { mantissa, scale })
    }

    /// Whether this is a whole number.
    ///
    /// A question with an exact answer now: `1.0` normalizes to a scale
    /// of zero and is one, `1.0000000000000001` does not and is not.
    pub(crate) fn is_integer(self) -> bool {
        self.scale >= 0
    }

    pub(crate) fn is_zero(self) -> bool {
        self.mantissa == 0
    }

    /// This number's mantissa scaled to `scale`, or `None` when that
    /// does not fit an `i128`.
    fn at_scale(self, scale: i32) -> Option<i128> {
        let steps = self.scale.checked_sub(scale)?;
        if steps < 0 {
            return None; // would lose digits rather than gain them
        }
        let steps = u32::try_from(steps).ok()?;
        if steps > MAX_DIGITS {
            return None;
        }
        self.mantissa.checked_mul(10_i128.checked_pow(steps)?)
    }

    /// Compare exactly, or say that the numbers are too large to.
    pub(crate) fn compare(self, other: Self) -> Option<Ordering> {
        // A difference in sign settles it without any scaling.
        let signs = self.mantissa.signum().cmp(&other.mantissa.signum());
        if signs != Ordering::Equal {
            return Some(signs);
        }
        let scale = self.scale.min(other.scale);
        Some(self.at_scale(scale)?.cmp(&other.at_scale(scale)?))
    }

    /// Whether this is an exact integer multiple of `step`, or `None`
    /// when the arithmetic does not fit.
    ///
    /// `value / step` is `(m1 / m2) × 10^(s1 - s2)`, so the whole
    /// question is one integer remainder once the power of ten has been
    /// folded into whichever side it belongs to.
    pub(crate) fn is_multiple_of(self, step: Self) -> Option<bool> {
        if step.is_zero() {
            return None;
        }
        if self.is_zero() {
            return Some(true);
        }
        let shift = self.scale.checked_sub(step.scale)?;
        let (numerator, denominator) = if shift >= 0 {
            let steps = u32::try_from(shift).ok()?;
            if steps > MAX_DIGITS {
                return None;
            }
            (
                self.mantissa.checked_mul(10_i128.checked_pow(steps)?)?,
                step.mantissa,
            )
        } else {
            // `shift.unsigned_abs()` rather than `-shift`, which
            // overflows for `i32::MIN` — reachable from `1e-2147483648`.
            let steps = shift.unsigned_abs();
            if steps > MAX_DIGITS {
                return None;
            }
            (
                self.mantissa,
                step.mantissa.checked_mul(10_i128.checked_pow(steps)?)?,
            )
        };
        // `i128::MIN % -1` overflows, though the remainder is plainly
        // zero; every other pair divides normally.
        Some(
            numerator
                .checked_rem(denominator)
                .is_none_or(|remainder| remainder == 0),
        )
    }

    /// This number as a JSON value, keeping its exact literal.
    ///
    /// A parameter arrives as text, so this is how it re-enters the
    /// document the schema will judge — without a detour through `f64`,
    /// which is the detour that loses everything.
    pub(crate) fn into_value(self) -> serde_json::Value {
        // `Display` writes a well-formed JSON number, which is what the
        // constructor requires of its caller.
        serde_json::Value::Number(serde_json::Number::from_string_unchecked(self.to_string()))
    }
}

impl Display for Decimal {
    /// Written back the way a person would write it, since these end up
    /// in error messages rather than in documents.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.scale == 0 {
            return write!(f, "{}", self.mantissa);
        }
        let digits = self.mantissa.unsigned_abs().to_string();
        let sign = if self.mantissa < 0 { "-" } else { "" };

        if self.scale > 0 {
            // Padding with more zeros than a reader can count helps
            // nobody; past that, say it in exponent form.
            if self.scale <= 21 {
                let zeros = "0".repeat(self.scale.unsigned_abs() as usize);
                return write!(f, "{sign}{digits}{zeros}");
            }
            return write!(f, "{sign}{digits}e{}", self.scale);
        }

        let places = self.scale.unsigned_abs() as usize;
        if places > 21 {
            return write!(f, "{sign}{digits}e{}", self.scale);
        }
        if places >= digits.len() {
            let leading = "0".repeat(places - digits.len());
            write!(f, "{sign}0.{leading}{digits}")
        } else {
            let (whole, fraction) = digits.split_at(digits.len() - places);
            write!(f, "{sign}{whole}.{fraction}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decimal(text: &str) -> Decimal {
        Decimal::parse(text).unwrap_or_else(|| panic!("{text} must parse"))
    }

    #[test]
    fn the_same_value_written_differently_is_the_same_decimal() {
        for spelling in ["1", "1.0", "1.00", "10e-1", "0.01e2", "+1", "01"] {
            assert_eq!(decimal(spelling), decimal("1"), "{spelling}");
        }
    }

    #[test]
    fn a_whole_number_is_recognised_however_it_was_spelled() {
        for spelling in ["1", "1.0", "100e-2", "0", "-0.0", "2e3"] {
            assert!(decimal(spelling).is_integer(), "{spelling}");
        }
        for spelling in ["1.5", "1.0000000000000001", "2251799813685248.25", "1e-3"] {
            assert!(!decimal(spelling).is_integer(), "{spelling}");
        }
    }

    #[test]
    fn comparison_is_exact_past_what_a_double_can_hold() {
        // The pair that a `f64` makes equal.
        assert_eq!(
            decimal("9007199254740993").compare(decimal("9007199254740992")),
            Some(Ordering::Greater),
        );
        // And the fractional bound that rounds to a whole number.
        assert_eq!(
            decimal("9007199254740994").compare(decimal("9007199254740993.5")),
            Some(Ordering::Greater),
        );
        assert_eq!(
            decimal("9007199254740993").compare(decimal("9007199254740993.5")),
            Some(Ordering::Less),
        );
    }

    #[test]
    fn comparison_settles_signs_without_scaling() {
        assert_eq!(
            decimal("-1e30").compare(decimal("1e-30")),
            Some(Ordering::Less)
        );
        assert_eq!(decimal("0").compare(decimal("-0.0")), Some(Ordering::Equal));
    }

    #[test]
    fn ordinary_decimals_compare_the_way_arithmetic_says() {
        assert_eq!(
            decimal("0.1").compare(decimal("0.1")),
            Some(Ordering::Equal)
        );
        assert_eq!(decimal("0.1").compare(decimal("0.2")), Some(Ordering::Less));
        assert_eq!(
            decimal("10").compare(decimal("9.9")),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn divisibility_is_decided_rather_than_divided() {
        // The case binary floating point cannot answer.
        assert_eq!(decimal("0.3").is_multiple_of(decimal("0.1")), Some(true));
        assert_eq!(decimal("0.35").is_multiple_of(decimal("0.1")), Some(false));
        assert_eq!(decimal("1.23").is_multiple_of(decimal("0.01")), Some(true));
        // And the one a rounded quotient got wrong.
        assert_eq!(
            decimal("2814749767106564").is_multiple_of(decimal("1.25")),
            Some(false),
        );
        assert_eq!(decimal("4").is_multiple_of(decimal("2")), Some(true));
        assert_eq!(decimal("5").is_multiple_of(decimal("2")), Some(false));
        assert_eq!(
            decimal("1").is_multiple_of(decimal("1.0000000000000001")),
            Some(false),
        );
    }

    #[test]
    fn zero_is_a_multiple_of_anything_and_nothing_is_a_multiple_of_zero() {
        assert_eq!(decimal("0").is_multiple_of(decimal("7")), Some(true));
        assert_eq!(decimal("0.0").is_multiple_of(decimal("1.5")), Some(true));
        assert_eq!(decimal("7").is_multiple_of(decimal("0")), None);
    }

    #[test]
    fn the_range_that_can_be_held_is_exactly_i128s() {
        // Both ends, which are not symmetric: `i128::MIN`'s magnitude is
        // one larger than `i128::MAX`, so building it positive and
        // negating would refuse it.
        let max = i128::MAX.to_string();
        let min = i128::MIN.to_string();
        assert!(Decimal::parse(&max).is_some(), "{max}");
        assert!(Decimal::parse(&min).is_some(), "{min}");

        // And one past each end.
        assert_eq!(Decimal::parse(&(i128::MAX as u128 + 1).to_string()), None);
        assert_eq!(
            Decimal::parse(&format!("-{}", i128::MIN.unsigned_abs() + 1)),
            None
        );

        // Both ends have 39 digits, so "39 digits" is not the boundary —
        // the range is.
        assert_eq!(max.len(), 39);
        assert!(Decimal::parse(&"9".repeat(38)).is_some());
        assert_eq!(Decimal::parse(&"9".repeat(39)), None);

        assert_eq!(Decimal::parse("1e999999999999"), None);
    }

    #[test]
    fn the_most_negative_number_behaves_like_any_other() {
        let min = decimal(&i128::MIN.to_string());
        assert_eq!(min.compare(decimal("0")), Some(Ordering::Less));
        assert_eq!(min.compare(min), Some(Ordering::Equal));
        assert!(min.is_integer());
        // `i128::MIN % -1` overflows though the answer is plainly zero.
        assert_eq!(min.is_multiple_of(decimal("-1")), Some(true));
        assert_eq!(min.is_multiple_of(decimal("1")), Some(true));
        assert_eq!(min.is_multiple_of(decimal("2")), Some(true));
        assert_eq!(min.to_string(), i128::MIN.to_string());
    }

    #[test]
    fn an_extreme_exponent_is_refused_rather_than_overflowing() {
        // Valid JSON, and every one of these used to run a scale past
        // `i32` — a panic in a checked build and a wrapped scale in a
        // release one.
        assert_eq!(Decimal::parse("10e2147483647"), None);
        assert_eq!(Decimal::parse("100e2147483646"), None);
        // The extreme that does fit is still read.
        assert!(Decimal::parse("1e2147483647").is_some());
        assert!(Decimal::parse("1e-2147483648").is_some());
    }

    #[test]
    fn divisibility_at_an_extreme_scale_does_not_overflow() {
        // `shift` reaches `i32::MIN` here, which cannot be negated.
        let tiny = decimal("1e-2147483648");
        assert_eq!(tiny.is_multiple_of(decimal("1")), None);
        assert_eq!(decimal("1").is_multiple_of(tiny), None);
    }

    #[test]
    fn what_is_not_a_number_does_not_parse() {
        for text in ["", "abc", "1.2.3", "1e", "--1", ".", "1e1e1", "0x10"] {
            assert_eq!(Decimal::parse(text), None, "{text}");
        }
    }

    #[test]
    fn a_decimal_writes_itself_the_way_it_was_meant() {
        for (text, shown) in [
            ("1", "1"),
            ("1.0", "1"),
            ("1.5", "1.5"),
            ("-2.25", "-2.25"),
            ("0.001", "0.001"),
            ("100", "100"),
            ("1e3", "1000"),
            ("9007199254740993", "9007199254740993"),
            ("0", "0"),
        ] {
            assert_eq!(decimal(text).to_string(), shown, "{text}");
        }
    }

    #[test]
    fn an_extreme_scale_falls_back_to_exponent_form() {
        assert_eq!(decimal("1e40").to_string(), "1e40");
        assert_eq!(decimal("1e-40").to_string(), "1e-40");
    }
}
