#!/bin/bash
# Installation script for octomind shell completions

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Try release binary first, then debug binary
if [[ -f "${SCRIPT_DIR}/../target/release/octomind" ]]; then
    OCTOMIND_BIN="${SCRIPT_DIR}/../target/release/octomind"
elif [[ -f "${SCRIPT_DIR}/../target/debug/octomind" ]]; then
    OCTOMIND_BIN="${SCRIPT_DIR}/../target/debug/octomind"
else
    echo "Error: octomind binary not found"
    echo "Please run 'cargo build --release' or 'cargo build' first"
    exit 1
fi

echo "Installing shell completions for octomind..."

# Detect the shell and install accordingly
detect_shell() {
    if [[ -n "$ZSH_VERSION" ]]; then
        echo "zsh"
    elif [[ -n "$BASH_VERSION" ]]; then
        echo "bash"
    else
        # Try to detect from SHELL environment variable
        case "$SHELL" in
            */zsh) echo "zsh" ;;
            */bash) echo "bash" ;;
            *) echo "unknown" ;;
        esac
    fi
}

install_bash_completion() {
    echo "Installing bash completion..."
    
    # Standard bash completion directories (in order of preference)
    BASH_COMPLETION_DIRS=(
        "$HOME/.local/share/bash-completion/completions"
        "$HOME/.bash_completion.d"
        "/usr/local/etc/bash_completion.d"
        "/etc/bash_completion.d"
    )
    
    # Find the first writable directory
    BASH_DIR=""
    for dir in "${BASH_COMPLETION_DIRS[@]}"; do
        if [[ -d "$(dirname "$dir")" ]] && [[ -w "$(dirname "$dir")" ]]; then
            BASH_DIR="$dir"
            break
        fi
    done
    
    if [[ -z "$BASH_DIR" ]]; then
        # Create user directory as fallback
        BASH_DIR="$HOME/.local/share/bash-completion/completions"
        mkdir -p "$BASH_DIR"
    else
        mkdir -p "$BASH_DIR"
    fi
    
    "$OCTOMIND_BIN" completion bash > "$BASH_DIR/octomind"
    echo "✓ Bash completion installed to: $BASH_DIR/octomind"
    
    # Check if bash-completion is properly configured
    if ! grep -q "bash-completion" "$HOME/.bashrc" 2>/dev/null && 
       ! grep -q "bash_completion" "$HOME/.bash_profile" 2>/dev/null; then
        echo ""
        echo "📝 To enable bash completion, add this to your ~/.bashrc:"
        echo "   # Enable bash completion"
        echo "   if [[ -f /usr/share/bash-completion/bash_completion ]]; then"
        echo "       source /usr/share/bash-completion/bash_completion"
        echo "   elif [[ -f /usr/local/etc/bash_completion ]]; then"
        echo "       source /usr/local/etc/bash_completion"
        echo "   fi"
        echo ""
        echo "   # Load user completions"
        echo "   if [[ -d ~/.local/share/bash-completion/completions ]]; then"
        echo "       for completion in ~/.local/share/bash-completion/completions/*; do"
        echo "           [[ -r \$completion ]] && source \$completion"
        echo "       done"
        echo "   fi"
    fi
}

install_zsh_completion() {
    echo "Installing zsh completion..."
    
    # Standard zsh completion directories (in order of preference)
    ZSH_COMPLETION_DIRS=(
        "$HOME/.local/share/zsh/site-functions"
        "$HOME/.zsh/completions"
        "$HOME/.config/zsh/completions"
        "/usr/local/share/zsh/site-functions"
        "/usr/share/zsh/site-functions"
    )
    
    # Find the first writable directory
    ZSH_DIR=""
    for dir in "${ZSH_COMPLETION_DIRS[@]}"; do
        if [[ -d "$(dirname "$dir")" ]] && [[ -w "$(dirname "$dir")" ]]; then
            ZSH_DIR="$dir"
            break
        fi
    done
    
    if [[ -z "$ZSH_DIR" ]]; then
        # Create user directory as fallback
        ZSH_DIR="$HOME/.local/share/zsh/site-functions"
        mkdir -p "$ZSH_DIR"
    else
        mkdir -p "$ZSH_DIR"
    fi
    
    "$OCTOMIND_BIN" completion zsh > "$ZSH_DIR/_octomind"
    echo "✓ Zsh completion installed to: $ZSH_DIR/_octomind"
    
    # Check if the directory is in fpath
    echo ""
    echo "📝 To enable zsh completion, ensure your ~/.zshrc contains:"
    echo "   # Add completion directory to fpath"
    echo "   fpath=($ZSH_DIR \$fpath)"
    echo "   autoload -U compinit && compinit"
    echo ""
    echo "   Alternatively, add this line to regenerate completions:"
    echo "   autoload -U compinit && compinit -d ~/.zcompdump"
    echo ""
    
    # Offer to fix common zsh completion issues
    if [[ -n "$ZSH_VERSION" ]]; then
        echo "🔧 Current session setup:"
        echo "   Run: autoload -U compinit && compinit -d ~/.zcompdump"
        echo "   Then: exec zsh  # to restart your shell"
    fi
}

# Main installation logic
SHELL_TYPE=$(detect_shell)

case "$1" in
    bash)
        install_bash_completion
        ;;
    zsh)
        install_zsh_completion
        ;;
    both|"")
        install_bash_completion
        install_zsh_completion
        ;;
    *)
        echo "Usage: $0 [bash|zsh|both]"
        echo "  bash - Install bash completion only"
        echo "  zsh  - Install zsh completion only" 
        echo "  both - Install both completions (default)"
        echo ""
        echo "Auto-detected shell: $SHELL_TYPE"
        exit 1
        ;;
esac

echo ""
echo "✅ Shell completion installation complete!"
echo ""
echo "💡 Quick test:"
echo "   octomind <TAB>        # Should show available commands"
echo "   octomind session <TAB> # Should show session options"
echo ""
echo "🔄 If completions don't work immediately:"
echo "   - Restart your shell: exec \$SHELL"
echo "   - Or source your config: source ~/.bashrc (bash) or source ~/.zshrc (zsh)"