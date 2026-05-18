{
  description = "Nix development environment for the Kern language workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
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
              clang
              cmake
              llvm.bintools
              llvm.clang
              llvm.lld
              llvm.llvm
              pkg-config
              rustc
              rustfmt
              clippy
              zlib
              zstd
            ] ++ lib.optionals stdenv.isDarwin [
              libiconv
            ] ++ lib.optionals stdenv.isLinux [
              libxml2
            ];

            env = {
              LLVM_SYS_211_PREFIX = llvmPrefix;
              KERN_TOOLCHAIN_ROOT = llvmPrefix;
              LIBCLANG_PATH = "${llvm.libclang.lib}/lib";
            };

            shellHook = ''
              export PATH="${llvm.clang}/bin:${llvm.lld}/bin:${llvm.llvm.dev}/bin:$PATH"
              echo "Kern dev shell ready"
              echo "LLVM_SYS_211_PREFIX=$LLVM_SYS_211_PREFIX"
            '';
          };
        });

      formatter = forEachSystem (pkgs: pkgs.nixfmt-rfc-style);
    };
}
