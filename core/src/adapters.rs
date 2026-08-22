use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackAdapter {
    pub id: &'static str,
    pub display_name: &'static str,
    pub file_markers: &'static [&'static str],
}

pub fn tier_a_stack_adapters() -> Vec<StackAdapter> {
    vec![
        StackAdapter {
            id: "rust",
            display_name: "Rust",
            file_markers: &["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"],
        },
        StackAdapter {
            id: "javascript-typescript",
            display_name: "JavaScript/TypeScript",
            file_markers: &["package.json", "tsconfig.json", "pnpm-lock.yaml"],
        },
        StackAdapter {
            id: "python",
            display_name: "Python",
            file_markers: &["pyproject.toml", "requirements.txt", "uv.lock"],
        },
        StackAdapter {
            id: "go",
            display_name: "Go",
            file_markers: &["go.mod", "go.sum"],
        },
        StackAdapter {
            id: "java",
            display_name: "Java",
            file_markers: &["pom.xml", "build.gradle", "settings.gradle"],
        },
        StackAdapter {
            id: "c-cpp",
            display_name: "C/C++",
            file_markers: &["CMakeLists.txt", "Makefile", "compile_commands.json"],
        },
        StackAdapter {
            id: "shell",
            display_name: "Shell",
            file_markers: &[".shellcheckrc"],
        },
        StackAdapter {
            id: "hcl",
            display_name: "HCL",
            file_markers: &["main.tf", "terraform.lock.hcl"],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_required_adapters() {
        let stacks: Vec<_> = tier_a_stack_adapters().into_iter().map(|a| a.id).collect();
        for required in [
            "rust",
            "javascript-typescript",
            "python",
            "go",
            "java",
            "c-cpp",
            "shell",
            "hcl",
        ] {
            assert!(stacks.contains(&required));
        }
    }
}
