//! 매크로 공용 유틸 — 이름 변환 · `:name` 파라미터 스캐너

/// CamelCase → snake_case (`TodoDao` → `todo_dao`)
pub fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// 자동 생성 SQL 식별자에 인용 종료 문자가 없는지 검증한다.
pub fn validate_sql_identifier(value: &str, span: proc_macro2::Span) -> syn::Result<()> {
    if value.contains('"') {
        return Err(syn::Error::new(
            span,
            "SQL 식별자에는 큰따옴표를 사용할 수 없습니다",
        ));
    }
    Ok(())
}

/// SQL에서 `:name` 파라미터 이름 추출 (등장 순서, 중복 제거).
/// 문자열 리터럴('…')·따옴표 식별자("…" `…` […])·라인 주석(--)·블록 주석은
/// 건너뛴다 (L-11).
pub fn extract_named_params(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // 문자열 리터럴 — '' 이스케이프 처리
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            // 따옴표 식별자
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                i += 1;
            }
            // 백틱 식별자 (MySQL 호환 표기 — SQLite 허용, L-11)
            b'`' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    i += 1;
                }
                i += 1;
            }
            // 대괄호 식별자 (MSSQL 호환 표기 — SQLite 허용, L-11)
            b'[' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                i += 1;
            }
            // 라인 주석
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // 블록 주석
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            // 이중 콜론(::) — 캐스트 표기 등, 파라미터 아님
            b':' if i + 1 < bytes.len() && bytes[i + 1] == b':' => {
                i += 2;
            }
            // 명명 파라미터
            b':' => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j > start {
                    let name = &sql[start..j];
                    if !out.iter().any(|n| n == name) {
                        out.push(name.to_string());
                    }
                }
                i = j;
            }
            prefix @ (b'@' | b'$') => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j > start {
                    let name = format!("{}{}", prefix as char, &sql[start..j]);
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
                i = j;
            }
            b'?' if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() => {
                let start = i;
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                let name = sql[start..j].to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// snake_case 변환 확인
    #[test]
    fn snake_case_basic() {
        assert_eq!(to_snake_case("TodoDao"), "todo_dao");
        assert_eq!(to_snake_case("UserPrefsDao"), "user_prefs_dao");
        assert_eq!(to_snake_case("Db"), "db");
    }

    /// 파라미터 추출 — 리터럴/주석/캐스트 무시
    #[test]
    fn params_extraction() {
        assert_eq!(
            extract_named_params("SELECT * FROM t WHERE a = :a AND b = ':not' AND c = :b -- :no"),
            vec!["a", "b"]
        );
        assert_eq!(extract_named_params("SELECT :x, :x, \":q\""), vec!["x"]);
    }

    /// 백틱·대괄호 식별자 안의 `:`는 파라미터가 아니다 (L-11)
    #[test]
    fn params_skip_quoted_identifiers() {
        assert_eq!(
            extract_named_params("SELECT `a:b`, [c:d] FROM t WHERE x = :x"),
            vec!["x"]
        );
        // 닫히지 않은 인용부 — 나머지 전체 스킵(파라미터 오탐 없음)
        assert!(extract_named_params("SELECT `a:b FROM t WHERE x = :x").is_empty());
    }

    /// 지원하지 않는 SQLite 파라미터 형식도 침묵 누락하지 않는다.
    #[test]
    fn unsupported_parameter_forms_are_detected() {
        assert_eq!(
            extract_named_params("SELECT @id, $name, ?12"),
            vec!["@id", "$name", "?12"]
        );
    }
}
