fn main() {
    // O D3DX9_43 vem do DirectX antigo e nem todo PC tem: carrega só quando um jogo D3D9 pede (install_d9).
    println!("cargo:rustc-cdylib-link-arg=/DELAYLOAD:D3DX9_43.dll");
    println!("cargo:rustc-cdylib-link-arg=delayimp.lib");
}
