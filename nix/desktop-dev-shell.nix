{ pkgs }:

let
  inherit (pkgs) lib;

  # Libraries linked directly by Buzz or loaded by Tauri/WebKitGTK at runtime.
  linuxLibraries = with pkgs; [
    alsa-lib
    dbus
    glib
    glib-networking
    gtk3
    libayatana-appindicator
    librsvg
    libsoup_3
    openssl
    webkitgtk_4_1
    xdotool
  ];
in
{
  packages = with pkgs; [
    pnpm
  ];

  nativeBuildInputs = with pkgs; [
    cmake
    pkg-config
  ];

  buildInputs = lib.optionals pkgs.stdenv.isLinux linuxLibraries;

  shellHook = lib.optionalString pkgs.stdenv.isLinux ''
    # Cargo-built development binaries are not wrapped like Nix packages, so
    # make Tauri's dynamically loaded GTK, WebKit, audio, and tray libraries
    # available when `tauri dev` launches the app.
    export LD_LIBRARY_PATH="${lib.makeLibraryPath linuxLibraries}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules''${GIO_EXTRA_MODULES:+:$GIO_EXTRA_MODULES}"
    if [[ -n "''${GSETTINGS_SCHEMAS_PATH:-}" ]]; then
      export XDG_DATA_DIRS="$GSETTINGS_SCHEMAS_PATH''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
    fi
  '';
}
