# devenv.nix
{ pkgs, ... }: {
  packages = [
    pkgs.rustup
    pkgs.espup
    pkgs.esp-generate
    pkgs.espflash
  ];

  enterShell = ''
    if [ -f "$HOME/export-esp.sh" ]; then
      . "$HOME/export-esp.sh"
    else
      echo "devenv: warning: ~/export-esp.sh not found — run 'espup install' to set up the Xtensa toolchain." >&2
    fi
  '';
}
