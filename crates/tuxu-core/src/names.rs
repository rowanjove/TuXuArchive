use crate::error::{err, Result};

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub fn validate_component(name: &str, kind: &str) -> Result<()> {
    if name.is_empty() {
        return Err(err("E005", format!("{kind}不能为空")));
    }
    if name != name.trim() || name.ends_with('.') || name.ends_with(' ') {
        return Err(err(
            "E005",
            format!("{kind}「{name}」含首尾空白或结尾句点，不能静默改写"),
        ));
    }
    if name == "." || name == ".." {
        return Err(err("E005", format!("{kind}「{name}」不是合法名称")));
    }
    if name.chars().count() > 200 {
        return Err(err("E005", format!("{kind}过长（最多 200 个字符）")));
    }
    if name
        .chars()
        .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
    {
        return Err(err(
            "E005",
            format!("{kind}「{name}」含 Windows 不允许的字符（<>:\"/\\|?*），请修改名称"),
        ));
    }
    let first = name.split('.').next().unwrap_or(name);
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(first)) {
        return Err(err(
            "E005",
            format!("{kind}「{name}」与 Windows 保留设备名冲突"),
        ));
    }
    Ok(())
}

pub fn sanitize_segment(raw: &str) -> String {
    let replaced: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "未命名".to_string()
    } else {
        let first = trimmed.split('.').next().unwrap_or(trimmed);
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(first)) {
            format!("{trimmed}_file")
        } else {
            trimmed.to_string()
        }
    }
}

pub fn stem_key(name: &str) -> String {
    name.to_lowercase()
}

pub fn parse_group_batch(text: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for raw in text.split([',', '，', ';', '；', '\n', '\r', '\t']) {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        validate_component(name, "分类名称")?;
        if out.iter().any(|e: &String| e.eq_ignore_ascii_case(name)) {
            return Err(err(
                "E001",
                format!("分类名称「{name}」在 Windows 下重复（不区分大小写）"),
            ));
        }
        out.push(name.to_string());
    }
    if out.is_empty() {
        return Err(err("", "没有解析到有效的分类名称"));
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Nat {
    Num(u128),
    Text(String),
}

fn natural_key(s: &str) -> Vec<Nat> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let num: String = chars[start..i].iter().collect();
            out.push(Nat::Num(num.parse().unwrap_or(0)));
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_ascii_digit() {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            out.push(Nat::Text(text.to_lowercase()));
        }
    }
    out
}

pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_group_batch_and_rejects_case_duplicates() {
        let err = parse_group_batch("风景, 美食，合影\n风景").unwrap_err();
        assert!(err.message.contains("风景"));
        let err_case = parse_group_batch("Travel, travel").unwrap_err();
        assert!(err_case.message.contains("重复"));
        let ok = parse_group_batch("01_风景, 02_美食；03_人物\n04_夜景").unwrap();
        assert_eq!(ok, vec!["01_风景", "02_美食", "03_人物", "04_夜景"]);
    }

    #[test]
    fn rejects_illegal_components() {
        assert!(validate_component("a:b", "分类名称").is_err());
        assert!(validate_component("CON", "分类名称").is_err());
        assert!(validate_component("尾部空格 ", "分类名称").is_err());
        assert!(validate_component("结尾句点.", "文件名").is_err());
        assert!(validate_component("2026-05-01_旅行相册", "分类名称").is_ok());
    }
}
