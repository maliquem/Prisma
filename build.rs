fn main() {
    println!("cargo:rerun-if-changed=res/prisma.ico");
    println!("cargo:rerun-if-changed=res/prisma.manifest");
    println!("cargo:rerun-if-changed=res/en.tsv");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("res/prisma.ico");
    res.set_manifest_file("res/prisma.manifest");
    res.compile().expect("falha ao compilar recursos");
    // Windows N não traz o Media Foundation: só a gravação fica de fora, o resto abre.
    // O dbghelp só é preciso se o programa cair, o WinHTTP ao procurar atualização, o mscms na aba HDR, o secur32 ao criar tarefas e o version depois de atualizar.
    for dll in ["mfplat.dll", "mf.dll", "mfreadwrite.dll", "dbghelp.dll", "winhttp.dll", "mscms.dll", "secur32.dll", "version.dll"] {
        println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg=delayimp.lib");
    // As DLLs da abertura vêm só da System32: na versão portátil, um version.dll baixado para a
    // mesma pasta rodaria dentro do Prisma. As carregadas depois ficam com o `main`.
    println!("cargo:rustc-link-arg=/DEPENDENTLOADFLAG:0x800");
    build_hook();
    build_shader_hook();
    build_apo();
}

/// O motor do equalizador (APO) também vai embutido. Sem `--locked`: o Cargo.lock do crate não é
/// guardado no repositório.
fn build_apo() {
    println!("cargo:rerun-if-changed=crates/prisma-apo/src");
    println!("cargo:rerun-if-changed=crates/prisma-apo/Cargo.toml");
    println!("cargo:rerun-if-changed=crates/prisma-eq/src");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("apo");
    let cargo = std::env::var("CARGO").unwrap();
    let done = std::process::Command::new(cargo)
        .args(["build", "--release", "--lib", "--manifest-path", "crates/prisma-apo/Cargo.toml", "--target-dir"])
        .arg(&out)
        .status()
        .expect("falha ao chamar o cargo");
    assert!(done.success(), "falha ao compilar o APO do equalizador");
}

/// O limitador é uma DLL própria, embutida no programa: tudo continua sendo um arquivo só.
fn build_hook() {
    println!("cargo:rerun-if-changed=hook/src");
    println!("cargo:rerun-if-changed=hook/Cargo.toml");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("hook");
    let cargo = std::env::var("CARGO").unwrap();
    let done = std::process::Command::new(cargo)
        .args(["build", "--release", "--locked", "--manifest-path", "hook/Cargo.toml", "--target-dir"])
        .arg(&out)
        .status()
        .expect("falha ao chamar o cargo");
    assert!(done.success(), "falha ao compilar o limitador");
}

/// Os shaders no modo Injeção: outra DLL, para o limitador continuar pequeno. Sai no mesmo perfil do programa
/// (debug no `cargo build`), sem `--locked`: o Cargo.lock dela não é guardado, como o do APO. Não vai dentro do
/// exe: fica em `target\<perfil>\injecao`, que a release zipa (`Prisma-injecao.zip`) e o debug usa direto.
fn build_shader_hook() {
    for p in ["shader-hook/src", "shader-hook/Cargo.toml", "shader-hook/vendor", "crates/prisma-shader/src", "crates/prisma-shader/Cargo.toml"] {
        println!("cargo:rerun-if-changed={p}");
    }
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("shader-hook");
    // A de 64 bits e, para os jogos de 32 bits, a mesma DLL em i686 com o ajudante que a prende no jogo
    // (precisa de `rustup target add i686-pc-windows-msvc`).
    for target in [None, Some("i686-pc-windows-msvc")] {
        let mut cmd = std::process::Command::new(std::env::var("CARGO").unwrap());
        cmd.args(["build", "--manifest-path", "shader-hook/Cargo.toml", "--target-dir"]).arg(&out);
        match target {
            Some(t) => cmd.args(["--target", t]),
            None => cmd.arg("--lib"),
        };
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            cmd.arg("--release");
        }
        let hint = if target.is_some() { " (falta o alvo? rode: rustup target add i686-pc-windows-msvc)" } else { "" };
        assert!(cmd.status().expect("falha ao chamar o cargo").success(), "falha ao compilar a DLL de shaders ({target:?}){hint}");
    }
    // shortcut: `out` é target\<perfil>\build\prisma-<hash>\out\shader-hook; trocar se o Cargo mudar esse formato.
    let profile = std::env::var("PROFILE").unwrap();
    let dest = out.ancestors().nth(4).expect("pasta do perfil").join("injecao");
    std::fs::create_dir_all(&dest).expect("pasta injecao");
    let i686 = out.join("i686-pc-windows-msvc").join(&profile);
    for (from, to) in [(out.join(&profile).join("prisma_shader_hook.dll"), "prisma_shader_hook.dll"), (i686.join("prisma_shader_hook.dll"), "prisma_shader_hook32.dll"), (i686.join("prisma_hook32.exe"), "prisma_hook32.exe")] {
        std::fs::copy(&from, dest.join(to)).unwrap_or_else(|e| panic!("copiar {}: {e}", from.display()));
    }
    std::fs::write(dest.join("versao.txt"), std::env::var("CARGO_PKG_VERSION").unwrap()).expect("versao.txt");
}
