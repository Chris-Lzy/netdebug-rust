//! 页面注册表。
//!
//! 加一个新页面只需要两步（详见 docs/ADD_PAGE.md）：
//! 1. 在 src/pages/ 里新建一个文件，实现 `Page` trait，并在下面加一行 `pub mod` 声明；
//! 2. 在下面的 `all_pages()` 里加一行。
//! 外壳（app.rs）不需要改，页签、拖出、收回、配置都会自动生效。

use crate::app::Shared;

pub mod debug;
pub mod master;
pub mod nic;
pub mod slave;
pub mod template;

/// 一个页面必须实现的东西：标识、标题、图标、绘制。
///
/// `show` 里只画界面，不直接碰网络/串口；需要后台能力时走 `shared.bus`
/// 发事件，业务逻辑以后再接。
pub trait Page {
    /// 全局唯一 id，只用小写字母和连字符，如 "debug"
    fn id(&self) -> &'static str;
    /// 页签上显示的中文名，如 "调试"
    fn title(&self) -> &'static str;
    /// 页签图标（单个字符/emoji），如 "⇄"
    fn icon(&self) -> &'static str;
    /// 绘制页面内容。`shared` 里是日志、配置、总线等共享状态。
    fn show(&mut self, ui: &mut egui::Ui, shared: &mut Shared);
}

/// 是否显示示例页（模板页）。默认关着；想看模板长什么样就改成 true。
pub const SHOW_TEMPLATE_PAGE: bool = false;

/// 注册表：顺序就是页签从左到右的顺序。
pub fn all_pages() -> Vec<Box<dyn Page>> {
    let mut pages: Vec<Box<dyn Page>> = vec![
        Box::new(crate::pages::debug::DebugPage::default()),
        Box::new(crate::pages::master::MasterPage::default()),
        Box::new(crate::pages::slave::SlavePage::default()),
        Box::new(crate::pages::nic::NicPage::default()),
    ];
    if SHOW_TEMPLATE_PAGE {
        pages.push(Box::new(crate::pages::template::TemplatePage::default()));
    }
    pages
}
