#!/bin/bash
# Installation script for octomind shell completions

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Use the most recently built binary (debug or release).
RELEASE="${SCRIPT_DIR}/../target/release/octomind"
DEBUG="${SCRIPT_DIR}/../target/debug/octomind"

if [[ -f "$DEBUG" && -f "$RELEASE" ]]; then
	if [[ "$DEBUG" -nt "$RELEASE" ]]; then
		OCTOMIND_BIN="$DEBUG"
	else
		OCTOMIND_BIN="$RELEASE"
	fi
elif [[ -f "$RELEASE" ]]; then
	OCTOMIND_BIN="$RELEASE"
elif [[ -f "$DEBUG" ]]; then
	OCTOMIND_BIN="$DEBUG"
else
	echo "Error: octomind binary not found"
	echo "Please run 'cargo build' first"
	exit 1
fi

echo "Installing shell completions for octomind..."

# Append lines to a file only if the guard comment is not already present.
append_if_absent() {
	local file="$1"
	local guard="$2"
	local content="$3"
	if grep -qF "$guard" "$file" 2>/dev/null; then
		return 0
	fi
	printf '\n%s\n' "$content" >> "$file"
}

install_bash_completion() {
	echo "Installing bash completion..."

	BASH_COMPLETION_DIRS=(
		"$HOME/.local/share/bash-completion/completions"
		"$HOME/.bash_completion.d"
		"/usr/local/etc/bash_completion.d"
		"/etc/bash_completion.d"
	)

	BASH_DIR=""
	for dir in "${BASH_COMPLETION_DIRS[@]}"; do
		if [[ -d "$(dirname "$dir")" ]] && [[ -w "$(dirname "$dir")" ]]; then
			BASH_DIR="$dir"
			break
		fi
	done

	if [[ -z "$BASH_DIR" ]]; then
		BASH_DIR="$HOME/.local/share/bash-completion/completions"
	fi
	mkdir -p "$BASH_DIR"

	"$OCTOMIND_BIN" completion bash > "$BASH_DIR/octomind"
	echo "✓ Bash completion file written to: $BASH_DIR/octomind"

	# Determine which rc file to update.
	# On macOS bash login shells read .bash_profile; interactive shells read .bashrc.
	# We write to whichever exists (preferring .bashrc, falling back to .bash_profile).
	local rc_file=""
	if [[ -f "$HOME/.bashrc" ]]; then
		rc_file="$HOME/.bashrc"
	elif [[ -f "$HOME/.bash_profile" ]]; then
		rc_file="$HOME/.bash_profile"
	else
		rc_file="$HOME/.bashrc"
		touch "$rc_file"
	fi

	local guard="# octomind completions"
	local snippet="${guard}
[[ -f \"${BASH_DIR}/octomind\" ]] && source \"${BASH_DIR}/octomind\""

	append_if_absent "$rc_file" "$guard" "$snippet"
	echo "✓ Shell config updated: $rc_file"
	echo "  Run: source $rc_file  (or open a new terminal)"
}

install_zsh_completion() {
	echo "Installing zsh completion..."

	# Use ~/.zsh/completions — a well-known user directory that works with
	# both plain zsh and oh-my-zsh (add it to fpath before compinit).
	ZSH_DIR="${HOME}/.zsh/completions"

	mkdir -p "$ZSH_DIR"
	chmod 755 "$ZSH_DIR"

	"$OCTOMIND_BIN" completion zsh > "$ZSH_DIR/_octomind"
	chmod 644 "$ZSH_DIR/_octomind"
	echo "✓ Zsh completion installed to: $ZSH_DIR/_octomind"

	# Clear the zsh completion dump so it is rebuilt with the new file.
	rm -f "${HOME}/.zcompdump"*
	echo "✓ Completion cache cleared"

	if [[ -d "${HOME}/.oh-my-zsh" ]]; then
		echo "  Open a new terminal to activate completions."
	else
		# Non-oh-my-zsh: ensure fpath and compinit are set in .zshrc.
		local rc_file="$HOME/.zshrc"
		[[ -f "$rc_file" ]] || touch "$rc_file"
		if grep -qF "$ZSH_DIR" "$rc_file" 2>/dev/null; then
			echo "  $ZSH_DIR already in $rc_file"
		else
			local guard="# octomind completions"
			local snippet="${guard}
fpath=(\"${ZSH_DIR}\" \$fpath)
autoload -Uz compinit && compinit -u"
			append_if_absent "$rc_file" "$guard" "$snippet"
			echo "✓ Added fpath to $rc_file"
		fi
		echo "  Run: source $rc_file  (or open a new terminal)"
	fi
}

# Main: detect shell and install both by default
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
		exit 1
		;;
esac

echo ""
echo "✅ Done. Open a new terminal (or source your shell config) to activate completions."
echo "   octomind run <TAB>  — should show agent tags and role names"
