{
  description = "clax — local artifacts with comment-driven development";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: rec {
        # The shell/bridge bundle and the Chrome extension the binary embeds
        # (clax-embedded's `CLAX_WEB_DIST_DIR` and `CLAX_EXTENSION_DIST_DIR`).
        # Both are gitignored, so a source-only build would embed empty
        # directories; build them here from web/'s pnpm lockfile. The source is
        # the repository, not web/ alone: the build reads ../Cargo.toml (the
        # extension's version) and ../../docs/legal (build-legal.mjs).
        clax-web = pkgs.stdenv.mkDerivation (finalAttrs: {
          pname = "clax-web";
          version = (pkgs.lib.importTOML ./Cargo.toml).workspace.package.version;
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./docs/legal
              (pkgs.lib.fileset.difference ./web (pkgs.lib.fileset.unions [
                (pkgs.lib.fileset.maybeMissing ./web/node_modules)
                (pkgs.lib.fileset.maybeMissing ./web/dist)
                (pkgs.lib.fileset.maybeMissing ./web/dist-extension)
              ]))
            ];
          };
          pnpmRoot = "web";
          pnpmDeps = pkgs.fetchPnpmDeps {
            inherit (finalAttrs) pname version src;
            sourceRoot = "${finalAttrs.src.name}/web";
            pnpm = pkgs.pnpm_12;
            fetcherVersion = 4;
            hash = "sha256-+i1lhsQ2b+xh1nHXAVFojs5f14x2ThcvqZfUZ5la988=";
          };
          nativeBuildInputs = [ pkgs.nodejs pkgs.pnpm_12 pkgs.pnpmConfigHook ];
          # `pnpm run build`: clean-dist, parts, bridge, shell, legal, extension.
          buildPhase = ''
            runHook preBuild
            (cd web && pnpm run build)
            runHook postBuild
          '';
          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp -r web/dist $out/dist
            cp -r web/dist-extension $out/dist-extension
            runHook postInstall
          '';
        });

        clax = pkgs.rustPlatform.buildRustPackage {
          pname = "clax";
          version = (pkgs.lib.importTOML ./Cargo.toml).workspace.package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "clax-cli" ];

          # rusqlite is `bundled` (needs a C compiler, which stdenv has); no
          # openssl: reqwest is default-features = false.
          CLAX_WEB_DIST_DIR = "${clax-web}/dist";
          CLAX_EXTENSION_DIST_DIR = "${clax-web}/dist-extension";

          # The workspace's gates (`just ci`) run the tests, some of which
          # bind ports and spawn daemons; a package build does not.
          doCheck = false;

          meta = {
            description = "clax: local artifacts with comment-driven development";
            mainProgram = "clax";
          };
        };

        default = clax;

        # The Claude Code marketplace tree, laid out as the repo lays it out
        # (`.claude-plugin/marketplace.json` -> `./plugins/claude-code`), so
        # the manifest's relative source resolves. This is what the module
        # links; `clax init` is NOT needed for Claude Code when this is used.
        claude-plugin = pkgs.runCommandLocal "clax-claude-plugin" { } ''
          mkdir -p $out/.claude-plugin $out/plugins
          cp ${./.claude-plugin/marketplace.json} $out/.claude-plugin/marketplace.json
          cp -r ${./plugins/claude-code} $out/plugins/claude-code
        '';
      });

      homeManagerModules.clax = import ./modules/clax.nix self;
      homeManagerModules.default = self.homeManagerModules.clax;
    };
}
