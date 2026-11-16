# 加一个新页面（三步）

## 第 1 步：复制模板

```sh
cp src/pages/template.rs src/pages/<新id>.rs
```

把文件里的 `TemplatePage` 改成自己的结构体名，`id()` / `title()` / `icon()` /
`show()` 换成自己的内容。`id` 只用小写字母和连字符，比如 `"monitor"`。

## 第 2 步：注册两行

`src/pages.rs` 顶部加一行：

```rust
pub mod <新id>;
```

`src/pages.rs` 的 `all_pages()` 里加一行（顺序就是页签从左到右的顺序）：

```rust
Box::new(crate::pages::<新id>::<新结构体>::default()),
```

## 第 3 步：编译运行

```sh
cargo run
```

新页签、拖出、收回、配置自动生效。

## 不要碰的东西

- `src/app.rs`（外壳）、`src/pages.rs` 的 trait 定义、`src/config.rs`、`src/log.rs`、
  `src/bus.rs`、`src/industrial.rs`——加页面不需要改它们。
- 页面里只画界面，不直接碰网络/串口；需要后台能力时走 `shared.bus` 发事件，
  或写 `shared.log` 记日志；没有后端的功能点按钮要弹“未接后端”提示。

## 想先看模板长什么样

把 `src/pages.rs` 里的 `SHOW_TEMPLATE_PAGE` 改成 `true`，运行后会出现一个“示例页”页签。
