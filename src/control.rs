//! 外部控制入口:托盘菜单、全局热键、信号。三者都把命令送进同一个 calloop 通道,
//! 事件循环那边只认 [`Control`],不关心命令是哪来的。

use std::collections::HashMap;

use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// 能从外部发起的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// 切换鼠标穿透。
    TogglePassthrough,
    /// 把宠物召回到屏幕中间(它跑到边角或看不见时用)。
    Recall,
    /// 切到进化链上的第几个形态(下标)。
    SwitchForm(usize),
    /// 退出。
    Quit,
}

/// 托盘图标 + 菜单。菜单动作发命令,穿透状态回显在勾选项上。
pub struct Tray {
    sender: Sender<Control>,
    passthrough: bool,
    pet_name: String,
    /// 进化链上的形态名(用于菜单),与 `current_form` 的下标对应。
    forms: Vec<String>,
    current_form: usize,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "rocom-pets".into()
    }

    fn title(&self) -> String {
        if self.pet_name.is_empty() {
            "rocom-pets".into()
        } else {
            format!("rocom-pets — {}", self.pet_name)
        }
    }

    /// 用主题里现成的图标名:自带 PNG 还要处理各种尺寸与主题深浅,收益不大。
    fn icon_name(&self) -> String {
        "face-smile".into()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            CheckmarkItem {
                label: "鼠标穿透".into(),
                checked: self.passthrough,
                activate: Box::new(|tray: &mut Self| tray.send(Control::TogglePassthrough)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "召回宠物".into(),
                icon_name: "go-home".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Control::Recall)),
                ..Default::default()
            }
            .into(),
        ];

        // 进化链有多个形态才给切换菜单(单形态的包不必多这一层)
        if self.forms.len() > 1 {
            items.push(
                SubMenu {
                    label: "形态".into(),
                    submenu: vec![
                        RadioGroup {
                            selected: self.current_form,
                            select: Box::new(|tray: &mut Self, index| {
                                tray.send(Control::SwitchForm(index))
                            }),
                            options: self
                                .forms
                                .iter()
                                .map(|name| RadioItem {
                                    label: name.clone(),
                                    ..Default::default()
                                })
                                .collect(),
                            ..Default::default()
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(ksni::MenuItem::Separator);
        items.push(
            StandardItem {
                label: "退出".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Control::Quit)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

impl Tray {
    fn send(&self, control: Control) {
        if self.sender.send(control).is_err() {
            log::warn!("主循环已退出,托盘命令没送出去");
        }
    }
}

/// 托盘句柄:拿着它才能更新勾选状态;drop 掉图标就消失。
pub struct TrayHandle(ksni::blocking::Handle<Tray>);

impl TrayHandle {
    /// 让菜单里的勾选跟上真实状态(穿透可能是热键或信号切的)。
    pub fn set_passthrough(&self, passthrough: bool) {
        self.0
            .update(move |tray: &mut Tray| tray.passthrough = passthrough);
    }

    /// 形态换了之后同步单选与标题。
    pub fn set_form(&self, index: usize, name: String) {
        self.0.update(move |tray: &mut Tray| {
            tray.current_form = index;
            tray.pet_name = name;
        });
    }
}

/// 起托盘。失败不致命(没有托盘宿主的桌面照样能跑),调用方只记个日志。
pub fn spawn_tray(
    sender: Sender<Control>,
    pet_name: String,
    passthrough: bool,
    forms: Vec<String>,
    current_form: usize,
) -> Result<TrayHandle> {
    use ksni::blocking::TrayMethods;
    let handle = Tray {
        sender,
        passthrough,
        pet_name,
        forms,
        current_form,
    }
    .spawn()
    .context("注册托盘图标失败(桌面没有 StatusNotifier 宿主?)")?;
    Ok(TrayHandle(handle))
}

/// 全局热键:走 XDG GlobalShortcuts portal。
///
/// 为什么不用 KGlobalAccel:那是 KDE 私有 D-Bus 接口,参数编码是 Qt 的 QKeySequence,
/// 而 portal 是跨桌面标准,KDE 自己也实现了。为什么不自己抓键:Wayland 下客户端根本
/// 抓不到输入区之外的按键——这正是要走 portal 的原因。
///
/// portal 的约定是「应用只能*建议*快捷键,最终由用户/桌面决定」,所以首次绑定时 KDE 会弹窗。
/// 整套流程都在独立线程里跑,失败只记日志:托盘菜单与 SIGUSR1 仍然可用。
pub fn spawn_hotkey(sender: Sender<Control>, trigger: String) {
    // BindShortcuts 会弹窗让用户确认,**在用户点之前不会回应**(实测 KDE 就是这样)。
    // 所以放个看门狗:久等没结果时提示去看弹窗,而不是让人以为程序卡住了
    let registered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = registered.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(10));
        if !watch.load(std::sync::atomic::Ordering::Relaxed) {
            log::warn!(
                "全局热键还没确认:GlobalShortcuts portal 通常会弹窗等你允许,去看一眼;\
                 不想用 portal 就用托盘菜单,或把 KDE 自定义快捷键绑到 \
                 `rocom-pets --toggle-passthrough`"
            );
        }
    });
    std::thread::spawn(move || {
        if let Err(e) = hotkey_thread(&sender, &trigger, &registered) {
            log::warn!("全局热键不可用({e:#});用托盘菜单或 D-Bus 命令代替");
        }
    });
}

fn hotkey_thread(
    sender: &Sender<Control>,
    trigger: &str,
    registered: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    let connection = Connection::session().context("连不上会话总线")?;
    let portal = Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.GlobalShortcuts",
    )
    .context("拿不到 GlobalShortcuts portal(桌面没实现?)")?;

    // portal 的每个调用都返回一个 Request 对象,真正的结果走它的 Response 信号。
    // 这里的做法:先订阅 Response,再发调用,避免竞争。
    let token = format!("rocom_pets_{}", std::process::id());
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("session_handle_token", Value::from(token.as_str()));
    let request: OwnedObjectPath = portal
        .call("CreateSession", &(options,))
        .context("CreateSession 失败")?;
    let session = wait_response(&connection, &request, "CreateSession")?
        .get("session_handle")
        .and_then(|v| String::try_from(v.clone()).ok())
        .context("CreateSession 没给 session_handle")?;
    let session = OwnedObjectPath::try_from(session).context("session_handle 不是对象路径")?;

    // 只申请一个动作:切换鼠标穿透。preferred_trigger 只是建议,用户可以改。
    let mut shortcut: HashMap<&str, Value> = HashMap::new();
    shortcut.insert("description", Value::from("切换鼠标穿透"));
    shortcut.insert("preferred_trigger", Value::from(trigger));
    let shortcuts = vec![("toggle-passthrough", shortcut)];
    let mut bind_options: HashMap<&str, Value> = HashMap::new();
    let bind_token = format!("{token}_bind");
    bind_options.insert("handle_token", Value::from(bind_token.as_str()));
    let request: OwnedObjectPath = portal
        .call("BindShortcuts", &(&session, shortcuts, "", bind_options))
        .context("BindShortcuts 失败")?;
    wait_response(&connection, &request, "BindShortcuts")?;
    registered.store(true, std::sync::atomic::Ordering::Relaxed);
    log::info!("全局热键已注册(建议 {trigger};KDE 里可在系统设置改键)");

    // 之后就是等 Activated 信号
    let session_path = session.as_str().to_string();
    let mut activated = zbus::blocking::MessageIterator::for_match_rule(
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.GlobalShortcuts")?
            .member("Activated")?
            .build(),
        &connection,
        None,
    )
    .context("订阅 Activated 失败")?;
    while let Some(message) = activated.next() {
        let message = message?;
        let body = message.body();
        // 信号体是 (session_handle, shortcut_id, timestamp, options)
        let parsed: zbus::Result<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)> =
            body.deserialize();
        let Ok((path, id, _timestamp, _options)) = parsed else {
            continue;
        };
        if path.as_str() != session_path {
            continue;
        }
        log::debug!("热键触发: {id}");
        if sender.send(Control::TogglePassthrough).is_err() {
            break; // 主循环退了
        }
    }
    Ok(())
}

/// 等某个 Request 的 Response 信号,返回结果字典。
fn wait_response(
    connection: &Connection,
    request: &OwnedObjectPath,
    what: &str,
) -> Result<HashMap<String, OwnedValue>> {
    let mut responses = zbus::blocking::MessageIterator::for_match_rule(
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.Request")?
            .member("Response")?
            .path(request.as_ref())?
            .build(),
        connection,
        None,
    )
    .with_context(|| format!("订阅 {what} 的 Response 失败"))?;
    let message = responses
        .next()
        .with_context(|| format!("{what} 没有回应"))??;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message
        .body()
        .deserialize()
        .with_context(|| format!("{what} 的 Response 解析失败"))?;
    // 0 = 成功,1 = 用户取消,2 = 其他错误
    anyhow::ensure!(code == 0, "{what} 被拒绝(code {code})");
    Ok(results)
}

// ── 自己的 D-Bus 控制接口 ───────────────────────────────────────────
// portal 那条路是能用的(KDE 会弹窗让用户确认,确认后就注册成功),但它有两个前提:
// 桌面得实现 GlobalShortcuts,用户得同意。这个 D-Bus 接口是补充,不是替代:
// - 不想走 portal 的人,可以在 KDE「自定义快捷键」里把任意键绑到
//   `rocom-pets --toggle-passthrough`,由这个接口通知常驻实例;
// - 顺带让宠物可脚本化(比如录屏脚本里先把它藏起来)。

const DBUS_NAME: &str = "org.rocom.Pets";
const DBUS_PATH: &str = "/org/rocom/Pets";

struct DbusControl {
    sender: std::sync::Mutex<Sender<Control>>,
}

#[zbus::interface(name = "org.rocom.Pets1")]
impl DbusControl {
    /// 切换鼠标穿透。
    fn toggle_passthrough(&self) {
        self.send(Control::TogglePassthrough);
    }

    /// 把宠物召回屏幕中间。
    fn recall(&self) {
        self.send(Control::Recall);
    }

    /// 退出。
    fn quit(&self) {
        self.send(Control::Quit);
    }
}

impl DbusControl {
    fn send(&self, control: Control) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(control);
        }
    }
}

/// 在会话总线上暴露控制接口。失败不致命(比如已经有一个实例占了名字)。
pub fn serve_dbus(sender: Sender<Control>) -> Result<()> {
    let control = DbusControl {
        sender: std::sync::Mutex::new(sender),
    };
    // 连接对象要一直活着,否则名字就丢了;这里故意 leak,进程退出时自然释放
    let connection = zbus::blocking::connection::Builder::session()
        .context("连不上会话总线")?
        .name(DBUS_NAME)
        .context("注册 D-Bus 名字失败(已经有一个实例在跑?)")?
        .serve_at(DBUS_PATH, control)
        .context("挂载 D-Bus 对象失败")?
        .build()
        .context("建立 D-Bus 连接失败")?;
    std::mem::forget(connection);
    log::info!("D-Bus 控制接口: {DBUS_NAME} {DBUS_PATH}(可用 --toggle-passthrough 等命令驱动)");
    Ok(())
}

/// 命令行子命令:通知已在运行的实例执行某个命令。
pub fn send_dbus_command(control: Control) -> Result<()> {
    let connection = Connection::session().context("连不上会话总线")?;
    let proxy = Proxy::new(&connection, DBUS_NAME, DBUS_PATH, "org.rocom.Pets1")
        .context("拿不到控制接口")?;
    let method = match control {
        Control::TogglePassthrough => "TogglePassthrough",
        Control::Recall => "Recall",
        Control::Quit => "Quit",
        // 换形态要带下标,命令行没暴露(托盘菜单里选更直观)
        Control::SwitchForm(_) => anyhow::bail!("换形态请用托盘菜单"),
    };
    proxy
        .call::<_, _, ()>(method, &())
        .with_context(|| format!("调用 {method} 失败(宠物没在跑?)"))?;
    Ok(())
}
