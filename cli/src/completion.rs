//! Shell completion script generation (bash / zsh / fish).

pub fn generate(shell: &str) -> anyhow::Result<&'static str> {
    match shell.to_lowercase().as_str() {
        "bash" => Ok(BASH),
        "zsh" => Ok(ZSH),
        "fish" => Ok(FISH),
        other => anyhow::bail!("unsupported shell '{other}'. Use: bash|zsh|fish"),
    }
}

const BASH: &str = r#"# ageverse bash completion — source or install under bash_completion.d
_ageverse() {
  local cur prev
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  prev="${COMP_WORDS[COMP_CWORD-1]}"

  local top_opts="-t --tui -p --instruction --model --permission --workdir --config --hooks --tool-mode --interactive-setup --dry-run -V --version --help"
  local top_cmds="eval config completion"
  local perm_modes="paranoid standard developer permissive yolo"
  local tool_modes="parallel sequential"

  if [[ ${COMP_CWORD} -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "${top_opts} ${top_cmds}" -- "${cur}") )
    return
  fi

  case "${COMP_WORDS[1]}" in
    eval)
      if [[ ${COMP_CWORD} -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "run" -- "${cur}") )
      else
        COMPREPLY=( $(compgen -W "-s --suite -m --mode --model --config -o --out --price-profile --gate --permission --max-iterations --variant --compare --ablate --help" -- "${cur}") )
      fi
      ;;
    config)
      if [[ ${COMP_CWORD} -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "show validate" -- "${cur}") )
      else
        COMPREPLY=( $(compgen -W "--config --probe --help" -- "${cur}") )
      fi
      ;;
    completion)
      COMPREPLY=( $(compgen -W "bash zsh fish" -- "${cur}") )
      ;;
    *)
      case "${prev}" in
        --permission) COMPREPLY=( $(compgen -W "${perm_modes}" -- "${cur}") ) ;;
        --tool-mode) COMPREPLY=( $(compgen -W "${tool_modes}" -- "${cur}") ) ;;
        -p|--instruction|--model|--workdir|--config) ;;
        *) COMPREPLY=( $(compgen -W "${top_opts} ${top_cmds}" -- "${cur}") ) ;;
      esac
      ;;
  esac
}
complete -F _ageverse ageverse
"#;

const ZSH: &str = r#"#compdef ageverse
# ageverse zsh completion — install as _ageverse on fpath

_ageverse() {
  local -a top_cmds
  top_cmds=(
    'eval:Run harness evaluation suites'
    'config:Show or validate configuration'
    'completion:Generate shell completion scripts'
  )

  local -a top_opts
  top_opts=(
    '(-t --tui)'{-t,--tui}'[launch TUI mode]'
    '(-p --instruction)'{-p,--instruction}'[one-shot prompt]:instruction:'
    '--model[model key from config]:model:'
    '--permission[permission mode]:mode:(paranoid standard developer permissive yolo)'
    '--workdir[working directory]:dir:_files -/'
    '--config[path to config.toml]:file:_files'
    '--hooks[enable logging hooks in REPL]'
    '--tool-mode[tool execution mode]:mode:(parallel sequential)'
    '--interactive-setup[ask setup questions at REPL startup]'
    '--dry-run[oneshot: call LLM but skip tool side effects]'
    '(-V --version)'{-V,--version}'[print version information]'
    '--help[display usage information]'
  )

  _arguments -C \
    $top_opts \
    '1:command:->cmds' \
    '*::arg:->args'

  case $state in
    cmds)
      _describe -t commands 'ageverse command' top_cmds
      ;;
    args)
      case $words[1] in
        eval)
          _arguments \
            '1:subcommand:(run)' \
            '(-s --suite)'{-s,--suite}'[suite name or path]:suite:' \
            '(-m --mode)'{-m,--mode}'[mock or live]:mode:(mock live)' \
            '--model[model key]:model:' \
            '--config[config path]:file:_files' \
            '(-o --out)'{-o,--out}'[output directory]:dir:_files -/' \
            '--price-profile[price table]:file:_files' \
            '--gate[fail on harness failures]' \
            '--permission[permission mode]:mode:(paranoid standard developer permissive yolo)' \
            '--max-iterations[max iterations]:n:' \
            '--variant[variant label]:label:' \
            '--compare[compare models]:models:' \
            '--ablate[ablation axes]:axes:'
          ;;
        config)
          _arguments \
            '1:subcommand:(show validate)' \
            '--config[config path]:file:_files' \
            '--probe[probe provider connectivity (validate only)]'
          ;;
        completion)
          _arguments '1:shell:(bash zsh fish)'
          ;;
      esac
      ;;
  esac
}

_ageverse "$@"
"#;

const FISH: &str = r#"# ageverse fish completion — save as ~/.config/fish/completions/ageverse.fish

complete -c ageverse -f
complete -c ageverse -s t -l tui -d 'launch TUI mode'
complete -c ageverse -s p -l instruction -d 'one-shot prompt' -r
complete -c ageverse -l model -d 'model key from config' -r
complete -c ageverse -l permission -d 'permission mode' -xa 'paranoid standard developer permissive yolo'
complete -c ageverse -l workdir -d 'working directory' -r
complete -c ageverse -l config -d 'path to config.toml' -r
complete -c ageverse -l hooks -d 'enable logging hooks in REPL'
complete -c ageverse -l tool-mode -d 'tool execution mode' -xa 'parallel sequential'
complete -c ageverse -l interactive-setup -d 'ask setup questions at REPL startup'
complete -c ageverse -l dry-run -d 'oneshot: call LLM but skip tool side effects'
complete -c ageverse -s V -l version -d 'print version information'
complete -c ageverse -l help -d 'display usage information'

complete -c ageverse -n '__fish_use_subcommand' -a eval -d 'Run harness evaluation suites'
complete -c ageverse -n '__fish_use_subcommand' -a config -d 'Show or validate configuration'
complete -c ageverse -n '__fish_use_subcommand' -a completion -d 'Generate shell completion scripts'

complete -c ageverse -n '__fish_seen_subcommand_from eval' -a run -d 'Execute an eval suite'
complete -c ageverse -n '__fish_seen_subcommand_from eval' -s s -l suite -r
complete -c ageverse -n '__fish_seen_subcommand_from eval' -s m -l mode -xa 'mock live'
complete -c ageverse -n '__fish_seen_subcommand_from eval' -l model -r
complete -c ageverse -n '__fish_seen_subcommand_from eval' -l config -r
complete -c ageverse -n '__fish_seen_subcommand_from eval' -s o -l out -r
complete -c ageverse -n '__fish_seen_subcommand_from eval' -l gate

complete -c ageverse -n '__fish_seen_subcommand_from config' -a show -d 'Print effective config (redacted)'
complete -c ageverse -n '__fish_seen_subcommand_from config' -a validate -d 'Validate config locally'
complete -c ageverse -n '__fish_seen_subcommand_from config' -l config -r
complete -c ageverse -n '__fish_seen_subcommand_from config' -l probe -d 'Probe provider (validate)'

complete -c ageverse -n '__fish_seen_subcommand_from completion' -a 'bash zsh fish'
"#;
