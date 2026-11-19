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
    println!("cargo:rerun-if-changed=hook/src/lib.rs");
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
/// (debug no `cargo build`), sem `--locked`: o Cargo.lock dela não é guardado, como o do APO.
fn build_shader_hook() {
    for p in ["shader-hook/src", "shader-hook/Cargo.toml", "crates/prisma-shader/src", "crates/prisma-shader/Cargo.toml"] {
        println!("cargo:rerun-if-changed={p}");
    }
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("shader-hook");
    let mut cmd = std::process::Command::new(std::env::var("CARGO").unwrap());
    cmd.args(["build", "--manifest-path", "shader-hook/Cargo.toml", "--target-dir"]).arg(&out);
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        cmd.arg("--release");
    }
    assert!(cmd.status().expect("falha ao chamar o cargo").success(), "falha ao compilar a DLL de shaders");
}
