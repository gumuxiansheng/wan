use crate::error::{Error, Result};
use crate::model::{EnvMap, Expr, Literal};

pub fn lookup<'a>(env: &'a EnvMap, key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

pub fn interpolate(s: &str, env: &EnvMap) -> (String, Vec<String>) {
    let mut out = String::with_capacity(s.len());
    let mut warnings = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = after[..end].trim();
                if name.is_empty() {
                    out.push_str("${}");
                } else if let Some(v) = lookup(env, name) {
                    out.push_str(v);
                } else {
                    out.push_str(&rest[start..start + 2 + end + 1]);
                    warnings.push(format!("未定义变量 ${{{name}}}，保留原样"));
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    (out, warnings)
}

fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn find_operator(s: &str) -> Option<usize> {
    let mut in_quote: Option<char> = None;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let c = bytes[i] as char;
        if let Some(q) = in_quote {
            if c == q {
                in_quote = None;
            }
        } else {
            match c {
                '"' | '\'' => in_quote = Some(c),
                '=' if bytes[i + 1] == b'=' => return Some(i),
                '!' if bytes[i + 1] == b'=' => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn parse_literal(s: &str) -> Result<Literal> {
    let t = s.trim();
    if t.len() >= 2 {
        let q = t.chars().next().unwrap();
        if q == '"' || q == '\'' {
            let inner = &t[1..];
            match inner.rfind(q) {
                Some(end) if end == inner.len() - 1 => {
                    return Ok(Literal::Str(inner[..end].to_string()))
                }
                _ => return Err(Error::config_at("literal 引号未闭合", 0, 0)),
            }
        }
    }
    match t {
        "true" | "True" | "TRUE" => return Ok(Literal::Bool(true)),
        "false" | "False" | "FALSE" => return Ok(Literal::Bool(false)),
        "null" | "Null" | "NULL" | "~" => return Ok(Literal::Null),
        _ => {}
    }
    let is_num = |s: &str| -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e' || c == 'E')
            && s.parse::<f64>().is_ok()
    };
    if is_num(t) {
        return Ok(Literal::Num(t.parse::<f64>().unwrap()));
    }
    Err(Error::config(format!(
        "非法的 if literal：`{t}`（支持 quoted_string / number / true / false / null）"
    )))
}

pub fn parse_if(s: &str) -> Result<Expr> {
    let t = s.trim();
    if t.is_empty() {
        return Err(Error::config("if 表达式为空"));
    }
    if t.contains("${{") {
        return Err(Error::config(
            "if 内不支持 `${{ }}`，本工具仅支持 ${VAR} 与受限 if 子集",
        ));
    }
    match t {
        "success()" => return Ok(Expr::Success),
        "failure()" => return Ok(Expr::Failure),
        "always()" => return Ok(Expr::Always),
        _ => {}
    }
    match find_operator(t) {
        Some(idx) => {
            let op = &t[idx..idx + 2];
            let var = t[..idx].trim();
            let lit = t[idx + 2..].trim();
            if var.is_empty() || lit.is_empty() {
                return Err(Error::config(format!("非法的 if 表达式：`{t}`")));
            }
            if !valid_ident(var) {
                return Err(Error::config(format!(
                    "if 比较式左操作数必须为标识符：`{var}`"
                )));
            }
            let literal = parse_literal(lit)?;
            Ok(match op {
                "==" => Expr::Eq(var.to_string(), literal),
                _ => Expr::Ne(var.to_string(), literal),
            })
        }
        None => Err(Error::config(format!(
            "非法的 if 表达式：`{t}`（支持 success()/failure()/always()/<var> == <literal>）"
        ))),
    }
}

pub fn literal_string(lit: &Literal) -> String {
    match lit {
        Literal::Str(s) => s.clone(),
        Literal::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
        Literal::Num(f) => {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Literal::Null => String::new(),
    }
}

/// `all_ok`: 全部依赖/先前 step 成功（无依赖时为 true）
/// `any_failed`: 任一依赖/先前 step 失败
pub fn eval(expr: &Expr, all_ok: bool, any_failed: bool, env: &EnvMap) -> bool {
    match expr {
        Expr::Success => all_ok,
        Expr::Failure => any_failed,
        Expr::Always => true,
        Expr::Eq(v, lit) => {
            let value = lookup(env, v).unwrap_or("").to_string();
            value == literal_string(lit)
        }
        Expr::Ne(v, lit) => {
            let value = lookup(env, v).unwrap_or("").to_string();
            value != literal_string(lit)
        }
    }
}

/// 求值上下文（spec §6.2 语义）：deps 为依赖 job / 先前 step 的结算状态
pub struct EvalCtx<'a> {
    pub all_ok: bool,
    pub any_failed: bool,
    pub env: &'a EnvMap,
}

impl<'a> EvalCtx<'a> {
    pub fn new(deps: &[crate::model::Outcome], env: &'a EnvMap) -> Self {
        EvalCtx {
            all_ok: deps.iter().all(|o| *o == crate::model::Outcome::Success),
            any_failed: deps.contains(&crate::model::Outcome::Failure),
            env,
        }
    }
}

impl Expr {
    pub fn eval(&self, ctx: &EvalCtx) -> bool {
        eval(self, ctx.all_ok, ctx.any_failed, ctx.env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> EnvMap {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn interpolate_basic() {
        let e = env(&[("HOME", "/root"), ("GLOBAL", "g")]);
        let (out, w) = interpolate("echo ${HOME}/build", &e);
        assert_eq!(out, "echo /root/build");
        assert!(w.is_empty());
    }

    #[test]
    fn interpolate_undefined_keeps_and_warns() {
        let e = env(&[]);
        let (out, w) = interpolate("echo ${NOPE}", &e);
        assert_eq!(out, "echo ${NOPE}");
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn interpolate_multi() {
        let e = env(&[("A", "1"), ("B", "2")]);
        let (out, _) = interpolate("${A}-${B}-${A}", &e);
        assert_eq!(out, "1-2-1");
    }

    #[test]
    fn parse_if_funcs() {
        assert_eq!(parse_if("success()").unwrap(), Expr::Success);
        assert_eq!(parse_if(" failure() ").unwrap(), Expr::Failure);
        assert_eq!(parse_if("always()").unwrap(), Expr::Always);
    }

    #[test]
    fn parse_if_comparison() {
        assert_eq!(
            parse_if("FOO == \"bar\"").unwrap(),
            Expr::Eq("FOO".into(), Literal::Str("bar".into()))
        );
        assert_eq!(
            parse_if("FOO != 3").unwrap(),
            Expr::Ne("FOO".into(), Literal::Num(3.0))
        );
        assert_eq!(
            parse_if("FLAG == true").unwrap(),
            Expr::Eq("FLAG".into(), Literal::Bool(true))
        );
        assert!(parse_if("FOO == \"unclosed").is_err());
        assert!(parse_if("FOO && BAR").is_err());
        assert!(parse_if("${{ x }}").is_err());
    }

    #[test]
    fn eval_semantics() {
        let e = env(&[("OS", "windows"), ("EMPTY", "")]);
        assert!(eval(&Expr::Success, true, false, &e));
        assert!(!eval(&Expr::Success, false, false, &e));
        assert!(eval(&Expr::Failure, true, true, &e));
        assert!(eval(&Expr::Always, false, true, &e));
        assert!(eval(&Expr::Eq("OS".into(), Literal::Str("windows".into())), true, false, &e));
        assert!(eval(&Expr::Eq("MISSING".into(), Literal::Str("".into())), true, false, &e));
        assert!(eval(&Expr::Eq("EMPTY".into(), Literal::Null), true, false, &e));
        assert!(eval(&Expr::Ne("OS".into(), Literal::Str("linux".into())), true, false, &e));
    }
}
