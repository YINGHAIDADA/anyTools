//! TypeScript → JavaScript by stripping types (oxc). No bundling, no type checking.

use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer};
use std::path::Path;

/// Wraps the script body in the async function it runs in. Done *before* parsing so that top-level
/// `await` and `return` are valid (and error positions stay on the same lines: the prefix adds none).
pub fn wrap(body: &str) -> String {
    format!("(async () => {{ {body}\n}})()")
}

/// Strips TypeScript syntax from an already wrapped script.
pub fn strip_types(wrapped: &str, path: &Path) -> Result<String, String> {
    let allocator = Allocator::default();
    let source_type = SourceType::ts().with_module(false);
    let ret = Parser::new(&allocator, wrapped, source_type).parse();
    if let Some(e) = ret.diagnostics.first() {
        return Err(format!("{} 语法错误: {e}", path.display()));
    }
    let mut program = ret.program;
    // enum_eval: the transformer needs constant-folded enum members to emit `enum` correctly.
    let scoping = SemanticBuilder::new().with_enum_eval(true).build(&program).semantic.into_scoping();
    // Only strip TypeScript; leave modern JS syntax alone (QuickJS supports ES2023).
    let options = TransformOptions::default();
    let ret = Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    if let Some(e) = ret.diagnostics.first() {
        return Err(format!("{} 转换失败: {e}", path.display()));
    }
    Ok(Codegen::new().build(&program).code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_types_keeps_code() {
        let js = strip_types(
            &wrap("interface P { x: number }\nconst f = (p: P): number => p.x * 2;\nlet s: string = `${f({ x: 2 })}`;\nenum E { A = 1 }\nreturn await Promise.resolve(s);"),
            Path::new("a.ts"),
        )
        .unwrap();
        assert!(!js.contains("interface") && !js.contains(": number"), "{js}");
        assert!(js.contains("p.x * 2") && js.contains("E") && js.contains("return await"), "{js}");
    }

    #[test]
    fn reports_syntax_errors_with_file() {
        let e = strip_types(&wrap("const = 1"), Path::new("x.ts")).unwrap_err();
        assert!(e.contains("x.ts") && e.contains("语法错误"), "{e}");
    }
}
