//! 示例页（模板）：新页面照这个抄。
//!
//! 加一个新页面只要三步（详见 docs/ADD_PAGE.md）：
//! 1. 复制本文件为 `src/pages/<新id>.rs`，把结构体名、`id()`、`title()`、
//!    `icon()` 和 `show()` 里的内容换成自己的；
//! 2. 在 `src/pages.rs` 顶部加一行 `pub mod <新id>;`；
//! 3. 在 `src/pages.rs` 的 `all_pages()` 里加一行
//!    `Box::new(crate::pages::<新id>::<新结构体>::default()),`。
//! 外壳（app.rs）不用动：页签、拖出、收回、配置自动生效。
//!
//! 想先看这个模板长什么样：把 `src/pages.rs` 里的
//! `SHOW_TEMPLATE_PAGE` 改成 `true` 即可。

use crate::app::Shared;
use crate::pages::Page;

/// 新页面的状态放这里。字段名用中文注释写清含义。
pub struct TemplatePage {
    /// 演示用的文本框
    text: String,
    /// 演示用的计数器
    counter: u32,
}

impl Default for TemplatePage {
    fn default() -> Self {
        Self {
            text: "你好，示例页".to_string(),
            counter: 0,
        }
    }
}

impl Page for TemplatePage {
    /// 全局唯一 id：只用小写字母和连字符。注册表和配置都用它来认页面。
    fn id(&self) -> &'static str {
        "template"
    }

    /// 页签上显示的中文名。
    fn title(&self) -> &'static str {
        "示例页"
    }

    /// 页签图标（单个字符）。
    fn icon(&self) -> &'static str {
        "☆"
    }

    /// 画页面内容。
    /// - 只画界面，不直接碰网络/串口；
    /// - 需要后台能力时走 `shared.bus` 发事件，或写 `shared.log` 记日志；
    /// - 没有后端的功能点按钮要弹“未接后端”提示，不假装已经能用。
    fn show(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        ui.heading("示例页");
        ui.label("这是一个新页面的模板：复制本文件，改 id/title/icon 和内容即可。");
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("文本");
            ui.text_edit_singleline(&mut self.text);
        });
        ui.horizontal(|ui| {
            if ui.button("计数 +1").clicked() {
                self.counter += 1;
            }
            ui.label(format!("计数：{}", self.counter));
        });
        if ui.button("发一条演示日志").clicked() {
            shared.log(crate::log::LogLevel::Sys, format!("示例页：{}", self.text));
        }
        if ui.button("弹未接后端提示").clicked() {
            shared.toast("未接后端：示例功能待实现");
        }
    }
}
