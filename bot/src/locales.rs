use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

use crate::parv_error;

pub type LocaleArg = BTreeMap<String, String>;

#[derive(Default)]
pub struct Locales {
    langs: Mutex<HashMap<String, HashMap<String, String>>>,
    default_lang: Mutex<String>,
}

static MISSING_HOOK: OnceLock<()> = OnceLock::new();

impl Locales {
    pub fn load(dir: &str) -> Result<Locales, String> {
        let mut langs = HashMap::new();
        let entries = std::fs::read_dir(dir).map_err(|e| format!("locales: {dir}: {e}"))?;
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("yml") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|x| x.to_str()) else {
                continue;
            };
            let text = std::fs::read_to_string(&p).map_err(|e| format!("locales: {stem}: {e}"))?;
            langs.insert(stem.to_string(), parse_yaml_flat(&text));
        }
        if langs.is_empty() {
            return Err(format!("locales: no yml in {dir}"));
        }
        Ok(Locales {
            langs: Mutex::new(langs),
            default_lang: Mutex::new("en".into()),
        })
    }

    pub fn set_default_lang(&self, lang: &str) {
        *self.default_lang.lock().unwrap() = lang.to_string();
    }

    pub fn default_lang(&self) -> String {
        self.default_lang.lock().unwrap().clone()
    }

    /// Sorted language codes whose yml vendored in.
    pub fn available(&self) -> Vec<String> {
        let mut codes: Vec<String> = self.langs.lock().unwrap().keys().cloned().collect();
        codes.sort();
        codes
    }

    pub fn keys(&self, lang: &str) -> usize {
        self.langs
            .lock()
            .unwrap()
            .get(lang)
            .map(|m| m.len())
            .unwrap_or(0)
    }

 /// Fallback chain: lang -> default_lang -> en; missing -> "[{key}]".
    pub fn get(&self, lang: &str, key: &str, args: &LocaleArg) -> String {
        let langs = self.langs.lock().unwrap();
        let def = self.default_lang();
        let chain = [Some(lang), Some(def.as_str()), Some("en")];
        for l in chain.into_iter().flatten() {
            if let Some(t) = langs.get(l).and_then(|m| m.get(key)) {
                return expand_emojis(&substitute(t, args));
            }
        }
        let _ = MISSING_HOOK.get_or_init(|| ());
        parv_error!("locales", "missing key {key}");
        format!("[{key}]")
    }
}

fn substitute(t: &str, args: &LocaleArg) -> String {
    let mut out = t.to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// {emoji:X} renders as the bare emoji (deviation from the legacy wrapper).
fn expand_emojis(t: &str) -> String {
    let bytes = t.as_bytes();
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && t[i..].starts_with("{emoji:") {
            if let Some(end) = t[i..].find('}') {
                out.push_str(&t[i + 7..i + end]);
                i += end + 1;
                continue;
            }
        }
        let ch = t[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn flush(
    key: &mut String,
    buf: &mut Vec<String>,
    mode: &mut char,
    map: &mut HashMap<String, String>,
) {
    if key.is_empty() || buf.is_empty() {
        key.clear();
        buf.clear();
        *mode = ' ';
        return;
    }
    let val = match *mode {
        '|' => buf.join("\n"),
        _ => buf.join(" "), // '>' folds newlines to spaces
    };
    let val = unquote(val.trim_end());
    map.insert(key.clone(), val);
    key.clear();
    buf.clear();
    *mode = ' ';
}

/// Flat-YAML parser:
/// `key:` at column 0; scalars quoted (both quote kinds) or plain;
/// block scalars `|` (keep newlines) and `>` (fold to spaces).
fn parse_yaml_flat(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut key = String::new();
    let mut buf: Vec<String> = Vec::new();
    let mut mode = ' '; // ' ' plain, '|' literal, '>' folded

    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let ind = line.len() - line.trim_start().len();
        if ind == 0 {
            flush(&mut key, &mut buf, &mut mode, &mut map);
            if let Some((k, rest)) = line.split_once(':') {
                key = k.trim().to_string();
                let v = rest.trim();
                if v == "|" || v == ">" {
                    mode = v.chars().next().unwrap();
                } else if !v.is_empty() {
                    map.insert(key.clone(), unquote(v));
                    key.clear();
                }
            }
        } else if !key.is_empty() {
            buf.push(line.trim().to_string());
        }
    }
    flush(&mut key, &mut buf, &mut mode, &mut map);
    map
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
        // YAML double-quoted scalars process escapes (\n \t \" \\);
        // full YAML parsing would do the same, so multiline keys must end
        // with real newlines here too.
        let inner = &v[1..v.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(cp) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(cp);
                    } else {
                        out.push('\\');
                        out.push('u');
                        out.push_str(&hex);
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        }
        return out;
    }
    if v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2 {
        // single-quoted YAML scalars only double the quote
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}
