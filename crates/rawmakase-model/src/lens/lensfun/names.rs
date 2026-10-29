//! Lens and camera names as the matcher compares them.
//!
//! A name is split into words of one kind of character, as Lensfun documents:
//! numbers (with a decimal point, "4.0" read as "4"), runs of letters, and single
//! punctuation characters. Case and word order don't matter; punctuation words are
//! dropped except `+` and `*`, which tell lenses apart, and so is a lone "f" (as in
//! "f/2.8"). Generations ("II", "Mark III", "Mk2") are read as one word, so "OSS II"
//! never matches "OSS". The focal lengths and aperture a name states ("18-55mm
//! f/3.5-5.6", "1:2/35") are read out too: two names that state different ones are
//! different lenses, whatever their words share.

/// A name's words, lower case.
pub fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = name.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || (chars[i] == '.'
                        && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                        && !chars[start..i].contains(&'.')))
            {
                i += 1;
            }
            out.push(number_word(&chars[start..i].iter().collect::<String>()));
            continue;
        }
        if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            out.push(chars[start..i].iter().collect::<String>().to_lowercase());
            continue;
        }
        if matches!(c, '+' | '*') {
            out.push(c.to_string());
        }
        i += 1;
    }
    out.retain(|w| w != "f");
    generations(out)
}

/// "4.0" as "4", "2.80" as "2.8".
fn number_word(text: &str) -> String {
    if text.contains('.') {
        let t = text.trim_end_matches('0').trim_end_matches('.');
        if t.is_empty() {
            "0".into()
        } else {
            t.to_string()
        }
    } else {
        let t = text.trim_start_matches('0');
        if t.is_empty() {
            "0".into()
        } else {
            t.to_string()
        }
    }
}

const ROMAN: [&str; 9] = ["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"];
/// A generation as `"#2"`: Roman numerals II to X, and "mark"/"mk" before a number or
/// Roman numeral (or joined to it, "mkii", "mk2").
fn generations(words: Vec<String>) -> Vec<String> {
    let roman = |w: &str| ROMAN.iter().position(|r| *r == w).map(|i| i + 2);
    let mut out = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let joined = w
            .strip_prefix("mark")
            .or_else(|| w.strip_prefix("mk"))
            .filter(|rest| !rest.is_empty())
            .and_then(roman);
        if let Some(n) = joined {
            out.push(format!("#{n}"));
            i += 1;
            continue;
        }
        if matches!(w, "mark" | "mk")
            && let Some(next) = words.get(i + 1)
        {
            let n =
                roman(next).or_else(|| next.parse::<usize>().ok().filter(|n| (2..=10).contains(n)));
            if let Some(n) = n {
                out.push(format!("#{n}"));
                i += 2;
                continue;
            }
        }
        // Tamron's "Di II" and "Di III" are lens lines, not generations.
        if i > 0
            && words[i - 1] == "di"
            && let Some(n) = roman(w)
        {
            out.pop();
            out.push(format!("di{n}"));
            i += 1;
            continue;
        }
        // "V" and "X" alone are letters ("1.4x", "VR"); only "Mark V" is a generation.
        // "I" after a word is the first ("OSS I/II" names both).
        match roman(w) {
            Some(n) if !matches!(w, "v" | "x") => out.push(format!("#{n}")),
            None if w == "i" && i > 0 => out.push("#1".into()),
            _ => out.push(w.to_string()),
        }
        i += 1;
    }
    out
}

/// The generations a name's words state; none states the first.
pub fn generation_words(words: &[String]) -> Vec<&str> {
    let mut g: Vec<&str> = words
        .iter()
        .filter(|w| w.starts_with('#'))
        .map(String::as_str)
        .collect();
    if g.is_empty() {
        g.push("#1");
    }
    g.sort_unstable();
    g.dedup();
    g
}

/// Whether a lens named with `lens`'s words can be of the generation `photo`'s words
/// state: Lensfun names one entry for several ("OSS I/II").
pub fn same_generation(photo: &[String], lens: &[String]) -> bool {
    let theirs = generation_words(lens);
    generation_words(photo).iter().all(|g| theirs.contains(g))
}

/// What a name says of the lens's focal lengths (mm) and widest aperture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stated {
    pub focal: Option<[f32; 2]>,
    pub aperture: Option<f32>,
}

/// The focal range and widest aperture `name` states: "70-200mm f/2.8", "18-55
/// 3.5-5.6", "50 f2", "F1.8", "1:2/35", "2.8-3.5/25-45".
pub fn stated(name: &str) -> Stated {
    // Any dash between numbers is a range ("35–80mm").
    let lower = name.to_lowercase().replace(
        [
            '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2212}',
        ],
        "-",
    );
    let mut s = Stated::default();
    // A number or a range of two: (first, last, byte range).
    let bytes = lower.as_bytes();
    let mut numbers: Vec<(f32, f32, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let read = |i: &mut usize| {
                let s0 = *i;
                while *i < bytes.len() && (bytes[*i].is_ascii_digit() || bytes[*i] == b'.') {
                    *i += 1;
                }
                lower[s0..*i].trim_end_matches('.').parse::<f32>().ok()
            };
            let Some(a) = read(&mut i) else {
                i += 1;
                continue;
            };
            let mut b = a;
            if i + 1 < bytes.len() && bytes[i] == b'-' && bytes[i + 1].is_ascii_digit() {
                let mut j = i + 1;
                if let Some(v) = read(&mut j) {
                    b = v;
                    i = j;
                }
            }
            numbers.push((a, b, start, i));
        } else {
            i += 1;
        }
    }
    let after = |end: usize, what: &str| lower[end..].trim_start().starts_with(what);
    let before = |start: usize, what: &str| lower[..start].trim_end().ends_with(what);
    for (k, &(a, b, start, end)) in numbers.iter().enumerate() {
        if after(end, "mm") && s.focal.is_none() {
            s.focal = Some([a, b]);
        } else if (before(start, "f/") || before(start, "f") || before(start, "1:"))
            && s.aperture.is_none()
            && a < 64.
        {
            s.aperture = Some(a);
        } else if after(end, "/") && numbers.get(k + 1).is_some_and(|n| n.2 == end + 1) {
            // "2.8-3.5/25-45": aperture, then focal lengths.
            s.aperture.get_or_insert(a);
            let n = numbers[k + 1];
            s.focal.get_or_insert([n.0, n.1]);
        }
    }
    // "18-55 3.5-5.6", "50 f2": a leading bare number or range is the focal length.
    if s.focal.is_none()
        && let Some(&(a, b, start, _)) = numbers.first()
        && !before(start, "f/")
        && !before(start, "1:")
        && !before(start, "f")
        && a >= 4.
    {
        s.focal = Some([a, b]);
    }
    s
}

impl Stated {
    /// Whether two names' statements can be the same lens.
    pub fn agree(&self, other: &Stated) -> bool {
        let close = |a: f32, b: f32| (a - b).abs() <= 0.03 * a.max(b) + 0.05;
        let focal = match (self.focal, other.focal) {
            (Some(a), Some(b)) => close(a[0], b[0]) && close(a[1], b[1]),
            _ => true,
        };
        let aperture = match (self.aperture, other.aperture) {
            (Some(a), Some(b)) => close(a, b),
            _ => true,
        };
        focal && aperture
    }
}

/// A maker's name as compared: lower case, without "corporation", "co.", "ltd." and
/// the like.
pub fn maker_words(maker: &str) -> Vec<String> {
    const NOISE: [&str; 12] = [
        "corporation",
        "corp",
        "co",
        "ltd",
        "inc",
        "company",
        "gmbh",
        "ag",
        "kg",
        "imaging",
        "camera",
        "optical",
    ];
    words(maker)
        .into_iter()
        .filter(|w| !NOISE.contains(&w.as_str()))
        .collect()
}

/// Lens makers that name lenses for other makers' mounts: a name that mentions one
/// of them is that maker's lens.
pub const THIRD_PARTY: [&str; 20] = [
    "sigma",
    "tamron",
    "tokina",
    "samyang",
    "rokinon",
    "walimex",
    "zeiss",
    "voigtlander",
    "voigtländer",
    "laowa",
    "venus",
    "viltrox",
    "yongnuo",
    "meike",
    "ttartisan",
    "7artisans",
    "irix",
    "mitakon",
    "zhongyi",
    "lensbaby",
];

/// Whether two camera or maker names are the same, ignoring case and spacing.
pub fn same_text(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// The focal range `name` states in millimetres ("70-200mm", "35–80mm"), as a range
/// read before any other number; `None` for a name that says no "mm" (a compact's
/// model number).
pub fn stated_millimetres(name: &str) -> Option<[f32; 2]> {
    let s = stated(name);
    let lower = name.to_lowercase();
    s.focal.filter(|_| lower.contains("mm"))
}
