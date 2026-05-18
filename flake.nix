{
  description = "Nix flake for the Kern language workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, ... }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem = f: lib.genAttrs systems (system: f (import nixpkgs { inherit system; }));
    in
    {
      devShells = forEachSystem (pkgs:
        let
          llvm = pkgs.llvmPackages_21;
          llvmPrefix = "${llvm.llvm.dev}";
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              cargo-edit
              cargo-nextest
              cmake
              pkg-config
              rustc
              rustfmt
              clippy
              llvm.bintools
              llvm.clang
              llvm.lld
              llvm.llvm
              zlib
              zstd
            ] ++ lib.optionals pkgs.stdenv.isDarwin [
              libiconv
            ] ++ lib.optionals pkgs.stdenv.isLinux [
              libxml2
            ];

            LLVM_SYS_211_PREFIX = llvmPrefix;
            KERN_TOOLCHAIN_ROOT = llvmPrefix;
            LIBCLANG_PATH = "${llvm.libclang.lib}/lib";
          };
        });

      packages = forEachSystem (pkgs:
        let
          llvm = pkgs.llvmPackages_21;
          llvmPrefix = "${llvm.llvm.dev}";
          runtimeLibPath = lib.makeLibraryPath (
            [ llvm.libclang.lib pkgs.zlib pkgs.zstd ]
            ++ lib.optionals pkgs.stdenv.isLinux [ pkgs.libxml2 ]
            ++ lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ]
          );

          bundleName = "kern";

          commonBuildInputs = [
            pkgs.zlib
            pkgs.zstd
          ] ++ lib.optionals pkgs.stdenv.isLinux [
            pkgs.libxml2
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
          ];

          commonNativeBuildInputs = [
            pkgs.cmake
            pkgs.makeWrapper
            pkgs.pkg-config
            llvm.bintools
            llvm.clang
            llvm.lld
            llvm.llvm
          ];

          mkKernPackage =
            {
              pname,
              cargoPackage,
              binaryName ? pname,
              extraBins ? [ ],
            }:
            pkgs.rustPlatform.buildRustPackage {
              inherit pname;
              version = "0.7.6";
              src = ./.;
              cargoLock.lockFile = ./Cargo.lock;

              buildAndTestSubdir = ".";
              cargoBuildFlags = [ "-p" cargoPackage ];
              doCheck = false;

              nativeBuildInputs = commonNativeBuildInputs;
              buildInputs = commonBuildInputs;

              LLVM_SYS_211_PREFIX = llvmPrefix;
              KERN_TOOLCHAIN_ROOT = llvmPrefix;
              LIBCLANG_PATH = "${llvm.libclang.lib}/lib";

              postInstall = ''
                mkdir -p "$out/lib/kern"
                cp -r library/* "$out/lib/kern/"

                wrapProgram "$out/bin/${binaryName}" \
                  --set-default KERNLIB_PATH "$out/lib/kern" \
                  --set-default KERN_CRAFT_SDK_ROOT "$out/lib/kern/craft" \
                  --set-default KERN_TOOLCHAIN_ROOT "${llvmPrefix}" \
                  --set-default LLVM_SYS_211_PREFIX "${llvmPrefix}" \
                  --set-default LIBCLANG_PATH "${llvm.libclang.lib}/lib" \
                  --prefix LD_LIBRARY_PATH : "${runtimeLibPath}" \
                  --prefix DYLD_LIBRARY_PATH : "${runtimeLibPath}"

              '' + lib.concatMapStringsSep "\n" (bin: ''
                wrapProgram "$out/bin/${bin}" \
                  --set-default KERNLIB_PATH "$out/lib/kern" \
                  --set-default KERN_CRAFT_SDK_ROOT "$out/lib/kern/craft" \
                  --set-default KERN_TOOLCHAIN_ROOT "${llvmPrefix}" \
                  --set-default LLVM_SYS_211_PREFIX "${llvmPrefix}" \
                  --set-default LIBCLANG_PATH "${llvm.libclang.lib}/lib" \
                  --prefix LD_LIBRARY_PATH : "${runtimeLibPath}" \
                  --prefix DYLD_LIBRARY_PATH : "${runtimeLibPath}"
              '') extraBins;

              meta = {
                description = "Kern language tool `${binaryName}`";
                license = lib.licenses.mit;
                platforms = systems;
              };
            };

          kernc = mkKernPackage {
            pname = "kernc";
            cargoPackage = "kernc_cli";
            binaryName = "kernc";
          };

          craft = mkKernPackage {
            pname = "craft";
            cargoPackage = "craft";
            binaryName = "craft";
          };

          kernLsp = mkKernPackage {
            pname = "kern-lsp";
            cargoPackage = "kern-lsp";
            binaryName = "kern-lsp";
          };

          default = pkgs.symlinkJoin {
            name = bundleName;
            paths = [
              kernc
              craft
              kernLsp
            ];
            buildInputs = [ pkgs.makeWrapper ];
            postBuild = ''
              wrapProgram "$out/bin/kernc" \
                --set-default KERNLIB_PATH "$out/lib/kern" \
                --set-default KERN_CRAFT_SDK_ROOT "$out/lib/kern/craft" \
                --set-default KERN_TOOLCHAIN_ROOT "${llvmPrefix}" \
                --set-default LLVM_SYS_211_PREFIX "${llvmPrefix}" \
                --set-default LIBCLANG_PATH "${llvm.libclang.lib}/lib" \
                --prefix LD_LIBRARY_PATH : "${runtimeLibPath}" \
                --prefix DYLD_LIBRARY_PATH : "${runtimeLibPath}"

              wrapProgram "$out/bin/craft" \
                --set-default KERNLIB_PATH "$out/lib/kern" \
                --set-default KERN_CRAFT_SDK_ROOT "$out/lib/kern/craft" \
                --set-default KERN_TOOLCHAIN_ROOT "${llvmPrefix}" \
                --set-default LLVM_SYS_211_PREFIX "${llvmPrefix}" \
                --set-default LIBCLANG_PATH "${llvm.libclang.lib}/lib" \
                --prefix LD_LIBRARY_PATH : "${runtimeLibPath}" \
                --prefix DYLD_LIBRARY_PATH : "${runtimeLibPath}"

              wrapProgram "$out/bin/kern-lsp" \
                --set-default KERNLIB_PATH "$out/lib/kern" \
                --set-default KERN_CRAFT_SDK_ROOT "$out/lib/kern/craft" \
                --set-default KERN_TOOLCHAIN_ROOT "${llvmPrefix}" \
                --set-default LLVM_SYS_211_PREFIX "${llvmPrefix}" \
                --set-default LIBCLANG_PATH "${llvm.libclang.lib}/lib" \
                --prefix LD_LIBRARY_PATH : "${runtimeLibPath}" \
                --prefix DYLD_LIBRARY_PATH : "${runtimeLibPath}"
            '';
          };
        in
        {
          inherit kernc craft default;
          kern-lsp = kernLsp;
        });

      checks = forEachSystem (pkgs:
        let
          system = pkgs.stdenv.hostPlatform.system;
          packages = self.packages.${system};
          mkVersionCheck = name: pkg:
            pkgs.runCommand "${name}-version-check" { } ''
              "${pkg}/bin/${name}" --version > /dev/null
              touch "$out"
            '';
        in
        {
          inherit (packages) kernc craft;
          kern-lsp = packages.kern-lsp;

          kernc-version = mkVersionCheck "kernc" packages.kernc;
          craft-version = mkVersionCheck "craft" packages.craft;
          kern-lsp-version = mkVersionCheck "kern-lsp" packages.kern-lsp;
        });

      overlays.default = final: _prev: {
        kernc = self.packages.${final.stdenv.hostPlatform.system}.kernc;
        craft = self.packages.${final.stdenv.hostPlatform.system}.craft;
        kern-lsp = self.packages.${final.stdenv.hostPlatform.system}.kern-lsp;
        kern = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      apps = forEachSystem (pkgs:
        let
          system = pkgs.stdenv.hostPlatform.system;
        in
        {
          kernc = {
            type = "app";
            program = "${self.packages.${system}.kernc}/bin/kernc";
            meta.description = "Run the Kern compiler";
          };
          craft = {
            type = "app";
            program = "${self.packages.${system}.craft}/bin/craft";
            meta.description = "Run the Kern package manager and build tool";
          };
          kern-lsp = {
            type = "app";
            program = "${self.packages.${system}.kern-lsp}/bin/kern-lsp";
            meta.description = "Run the Kern language server";
          };
          default = self.apps.${system}.kernc // {
            meta.description = "Run the default Kern CLI entrypoint";
          };
        });

      formatter = forEachSystem (pkgs: pkgs.nixfmt);
    };
}
