use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackAdapter {
    pub id: &'static str,
    pub display_name: &'static str,
    pub file_markers: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtocolAdapter {
    pub id: &'static str,
    pub display_name: &'static str,
    /// Which crate/command implements this adapter (or "experimental").
    pub scope: &'static str,
    /// Honest status: "implemented" or "experimental" (no misleading stubs).
    pub status: &'static str,
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

pub fn protocol_adapters() -> Vec<ProtocolAdapter> {
    vec![
        ProtocolAdapter {
            id: "mcp",
            display_name: "Model Context Protocol",
            scope: "draft-adapters (draft mcp)",
            status: "implemented",
        },
        ProtocolAdapter {
            id: "acp-client",
            display_name: "Agent Client Protocol",
            scope: "draft-adapters (draft acp)",
            status: "implemented",
        },
        ProtocolAdapter {
            id: "acp-comm",
            display_name: "Agent Communication Protocol",
            scope: "draft-adapters (draft acp)",
            status: "experimental",
        },
        ProtocolAdapter {
            id: "a2a",
            display_name: "Agent2Agent",
            scope: "draft-adapters (draft a2a)",
            status: "implemented",
        },
        ProtocolAdapter {
            id: "ag-ui",
            display_name: "AG-UI",
            scope: "draft-agui (draft cockpit)",
            status: "implemented",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_real_or_experimental_adapters() {
        // No adapter may claim stub behavior as working.
        for a in protocol_adapters() {
            assert!(
                a.status == "implemented" || a.status == "experimental",
                "{} has misleading status {}",
                a.id,
                a.status
            );
        }
    }

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

        let protocols: Vec<_> = protocol_adapters().into_iter().map(|a| a.id).collect();
        for required in ["mcp", "acp-client", "acp-comm", "a2a", "ag-ui"] {
            assert!(protocols.contains(&required));
        }
    }
}
