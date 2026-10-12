//! Shell completion scripts printed by `--zsh-complete`.

/// zsh completion script. `@BIN@` is replaced with the binary name.
///
/// Kept in sync with `src/cli.rs`: option names and help strings must match.
/// Dynamic values (models, roles, sessions, agents, rags, macros) come from
/// the corresponding `--list-*` flags.
const ZSH: &str = r#"#compdef @BIN@

autoload -U is-at-least

_@BIN@() {
    typeset -A opt_args
    typeset -a _arguments_options
    local ret=1

    if is-at-least 5.2; then
        _arguments_options=(-s -S -C)
    else
        _arguments_options=(-s -C)
    fi

    local context curcontext="$curcontext" state line
    local common=(
'-m[Select a LLM model]:MODEL:->models' \
'--model[Select a LLM model]:MODEL:->models' \
'--prompt[Use the system prompt]:PROMPT: ' \
'-r[Select a role]:ROLE:->roles' \
'--role[Select a role]:ROLE:->roles' \
'-s[Start or join a session]:SESSION:->sessions' \
'--session[Start or join a session]:SESSION:->sessions' \
'--empty-session[Ensure the session is empty]' \
'--save-session[Ensure the new conversation is saved to the session]' \
'-a[Start a agent]:AGENT:->agents' \
'--agent[Start a agent]:AGENT:->agents' \
'--agent-variable[Set agent variables]' \
'--rag[Start a RAG]:RAG:->rags' \
'--rebuild-rag[Rebuild the RAG to sync document changes]' \
'--macro[Execute a macro]:MACRO:->macros' \
'--serve[Serve the LLM API and WebAPP]' \
'-e[Execute commands in natural language]' \
'--execute[Execute commands in natural language]' \
'-c[Output code only]' \
'--code[Output code only]' \
'*-f[Include files, directories, or URLs]:FILE:_files' \
'*--file[Include files, directories, or URLs]:FILE:_files' \
'-S[Turn off stream mode]' \
'--no-stream[Turn off stream mode]' \
'--hide-think[Hide the thinking content in the output]' \
'--show-usage[Show a usage line after each response]' \
'--no-show-usage[Turn off the usage line after each response]' \
'--dry-run[Display the message without sending it]' \
'--info[Display information]' \
'--sync-models[Sync models updates]' \
'--list-models[List all available chat models]' \
'--list-roles[List all roles]' \
'--list-sessions[List all sessions]' \
'--list-agents[List all agents]' \
'--list-rags[List all RAGs]' \
'--list-macros[List all macros]' \
'--zsh-complete[Generate the zsh completion script]' \
'-h[Print help]' \
'--help[Print help]' \
'-V[Print version]' \
'--version[Print version]' \
'*::text -- Input text:' \
    )

    _arguments "${_arguments_options[@]}" $common \
        && ret=0
    case $state in
        models|roles|sessions|agents|rags|macros)
            local -a values expl
            values=( ${(f)"$(_call_program values @BIN@ --list-$state)"} )
            _wanted values expl $state compadd -a values && ret=0
            ;;
    esac
    return ret
}

if [ "$funcstack[1]" = "_@BIN@" ]; then
    _@BIN@ "$@"
else
    compdef _@BIN@ @BIN@
fi
"#;

/// The zsh completion script for this binary.
pub fn zsh() -> String {
    ZSH.replace("@BIN@", env!("CARGO_PKG_NAME"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zsh_completion_script() {
        let script = zsh();
        assert!(
            !script.contains("@BIN@"),
            "binary name placeholder left behind"
        );
        assert!(script.starts_with("#compdef aichat-ng\n"));
        assert!(script.contains("compdef _aichat-ng aichat-ng"));
        // options must be offered, including the fork's own
        for opt in [
            "--model",
            "--role",
            "--session",
            "--agent",
            "--rag",
            "--macro",
            "--file",
            "--hide-think",
            "--show-usage",
            "--no-show-usage",
            "--zsh-complete",
        ] {
            assert!(script.contains(opt), "missing option {opt}");
        }
        // dynamic values are fetched from --list-*
        assert!(script.contains("--list-$state"));
    }
}
