#!/bin/sh
set -eu

REPO="symbolicvic/agent-atuin"
BINARY_NAME="atuin"

cat << 'EOF'
 _______  _______  __   __  ___   __    _
|   _   ||       ||  | |  ||   | |  |  | |
|  |_|  ||_     _||  | |  ||   | |   |_| |
|       |  |   |  |  |_|  ||   | |       |
|       |  |   |  |       ||   | |  _    |
|   _   |  |   |  |       ||   | | | |   |
|__| |__|  |___|  |_______||___| |_|  |__|

Agent-Friendly Atuin Fork
Magical shell history with agent integration

https://github.com/symbolicvic/agent-atuin

===============================================================================

EOF

# Detect OS and architecture
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)
            case "$ARCH" in
                x86_64)
                    PLATFORM="x86_64-unknown-linux-gnu"
                    ;;
                aarch64|arm64)
                    PLATFORM="aarch64-unknown-linux-gnu"
                    ;;
                *)
                    echo "Unsupported architecture: $ARCH"
                    exit 1
                    ;;
            esac
            ;;
        Darwin)
            case "$ARCH" in
                x86_64)
                    PLATFORM="x86_64-apple-darwin"
                    ;;
                arm64)
                    PLATFORM="aarch64-apple-darwin"
                    ;;
                *)
                    echo "Unsupported architecture: $ARCH"
                    exit 1
                    ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            PLATFORM="x86_64-pc-windows-msvc"
            BINARY_NAME="atuin.exe"
            ;;
        *)
            echo "Unsupported operating system: $OS"
            exit 1
            ;;
    esac
}

# Get the latest release version
get_latest_version() {
    curl -sSL "https://api.github.com/repos/${REPO}/releases/latest" | \
        grep '"tag_name":' | \
        sed -E 's/.*"([^"]+)".*/\1/'
}

# Download and install the binary
install_binary() {
    VERSION="$1"
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/atuin-${PLATFORM}.tar.gz"

    echo "Downloading atuin ${VERSION} for ${PLATFORM}..."

    # Create install directory
    INSTALL_DIR="${HOME}/.local/bin"
    mkdir -p "$INSTALL_DIR"

    # Download and extract
    TEMP_DIR="$(mktemp -d)"
    cd "$TEMP_DIR"

    if command -v curl > /dev/null; then
        curl -sSL "$DOWNLOAD_URL" -o atuin.tar.gz
    elif command -v wget > /dev/null; then
        wget -q "$DOWNLOAD_URL" -O atuin.tar.gz
    else
        echo "Error: curl or wget required"
        exit 1
    fi

    tar -xzf atuin.tar.gz
    mv "$BINARY_NAME" "$INSTALL_DIR/"
    chmod +x "$INSTALL_DIR/$BINARY_NAME"

    cd - > /dev/null
    rm -rf "$TEMP_DIR"

    echo "Installed atuin to $INSTALL_DIR/$BINARY_NAME"

    # Add to PATH if needed
    case ":$PATH:" in
        *":$INSTALL_DIR:"*)
            ;;
        *)
            echo ""
            echo "Add the following to your shell profile to add atuin to your PATH:"
            echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
            ;;
    esac
}

# Setup shell integration
setup_shell() {
    # Zsh
    if [ -f "${ZDOTDIR:-$HOME}/.zshrc" ]; then
        if ! grep -q "atuin init zsh" "${ZDOTDIR:-$HOME}/.zshrc"; then
            printf '\neval "$(atuin init zsh)"\n' >> "${ZDOTDIR:-$HOME}/.zshrc"
            echo "Added atuin init to ~/.zshrc"
        fi
    fi

    # Bash
    if [ -f "$HOME/.bashrc" ]; then
        if ! grep -q "atuin init bash" "$HOME/.bashrc"; then
            # Install bash-preexec if not present
            if [ ! -f "$HOME/.bash-preexec.sh" ]; then
                curl -sSL https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o "$HOME/.bash-preexec.sh"
            fi
            printf '\n[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh\n' >> "$HOME/.bashrc"
            printf 'eval "$(atuin init bash)"\n' >> "$HOME/.bashrc"
            echo "Added atuin init to ~/.bashrc"
        fi
    fi

    # Fish
    if [ -f "$HOME/.config/fish/config.fish" ]; then
        if ! grep -q "atuin init fish" "$HOME/.config/fish/config.fish"; then
            printf '\natuin init fish | source\n' >> "$HOME/.config/fish/config.fish"
            echo "Added atuin init to ~/.config/fish/config.fish"
        fi
    fi
}

# Main installation
main() {
    if ! command -v curl > /dev/null && ! command -v wget > /dev/null; then
        echo "Error: curl or wget required"
        exit 1
    fi

    detect_platform
    VERSION="$(get_latest_version)"

    if [ -z "$VERSION" ]; then
        echo "Error: Could not determine latest version"
        exit 1
    fi

    install_binary "$VERSION"
    setup_shell

    cat << 'EOF'



 _______  __   __  _______  __    _  ___   _    __   __  _______  __   __
|       ||  | |  ||   _   ||  |  | ||   | | |  |  | |  ||       ||  | |  |
|_     _||  |_|  ||  |_|  ||   |_| ||   |_| |  |  |_|  ||   _   ||  | |  |
  |   |  |       ||       ||       ||      _|  |       ||  | |  ||  |_|  |
  |   |  |       ||       ||  _    ||     |_   |_     _||  |_|  ||       |
  |   |  |   _   ||   _   || | |   ||    _  |    |   |  |       ||       |
  |___|  |__| |__||__| |__||_|  |__||___| |_|    |___|  |_______||_______|


Thanks for installing agent-atuin!

For AI agents, set the ATUIN_AGENT_ID environment variable:
  export ATUIN_AGENT_ID="your-agent-name"

Key commands for agents:
  atuin search --json "pattern"           # Search history with JSON output
  atuin history list --json               # List history with JSON output
  atuin memory create "description"       # Create a memory
  atuin memory search "keyword" --json    # Search memories

For full documentation, see:
  https://github.com/symbolicvic/agent-atuin/blob/main/docs/AGENT_SETUP.md

EOF
}

main "$@"
