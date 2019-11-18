{ pkgs ? import <nixpkgs> {}, unstable ? import <nixos-unstable> {} }:
pkgs.mkShell {
  buildInputs = [
    ((pkgs.rustChannelOf { channel = "stable"; }).rust.override {
      # rustfmt on stable does not do match block trailing commas
      extensions = [ "clippy-preview" ]; # "rustfmt-preview" ];
    })

    pkgs.openssl
    pkgs.pkgconfig
    pkgs.gcc
    pkgs.rr
  ];

  shellHook = ''
    # path of this shell.nix file, escaped by systemd to have a working directory name identifier
    identifier=$(${pkgs.systemd}/bin/systemd-escape -p ${toString ./.})
    # all missing directories in $CARGO_TARGET_DIR path are created automatically by cargo
    export CARGO_TARGET_DIR="''${XDG_CACHE_HOME:-$HOME/.cache}/cargo/targets/$identifier"
  '';
}
