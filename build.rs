//! 打包时把 app.manifest（requireAdministrator）嵌进 exe。
//!
//! Chris 用 GNU 工具链（stable-x86_64-pc-windows-gnu + mingw64 的 windres）
//! 编译：build.rs 调 windres 把清单编成 .res，再让链接器链进去。
//! windres 不在（比如 Linux 上 cargo check）就跳过，不报错——
//! 清单只影响 Windows 上的提权，Linux 编译不需要它。
//! 注意：改了 manifest 要整包重编才会生效，cargo build --release 即可。

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let manifest = std::path::Path::new(&manifest_dir).join("app.manifest");
    if !manifest.exists() {
        println!("cargo:warning=app.manifest 不存在，跳过管理员清单嵌入");
        return;
    }
    let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
    let out_path = std::path::Path::new(&out_dir);
    // rc 文件放 OUT_DIR，里面写清单的绝对路径（windres 按当前目录解析相对路径，不可靠）。
    let rc = out_path.join("app.rc");
    let manifest_str = manifest.to_string_lossy().replace('\\', "/");
    if std::fs::write(&rc, format!("1 24 \"{manifest_str}\"\n")).is_err() {
        println!("cargo:warning=写 app.rc 失败，跳过管理员清单嵌入");
        return;
    }
    let res = out_path.join("app.res");
    let status = std::process::Command::new("windres")
        .arg(rc.to_str().unwrap())
        .args(["-O", "coff", "-o"])
        .arg(res.to_str().unwrap())
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg={}", res.display());
        }
        _ => {
            println!("cargo:warning=windres 不可用或失败，跳过管理员清单嵌入（netsh 改 IP 需要管理员权限，请用管理员身份运行 exe）");
        }
    }
}
