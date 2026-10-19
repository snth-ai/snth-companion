//! The few strings the shell itself shows (tray menu, notifications).
//! Everything else is in the web UI (`app/src/i18n`).

use connect_core::LinkState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    pub fn parse(s: &str) -> Lang {
        if s.to_ascii_lowercase().starts_with("ru") {
            Lang::Ru
        } else {
            Lang::En
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }
}

pub fn tray_status(lang: Lang, state: LinkState, paired: bool, paused: bool, synth: &str) -> String {
    if !paired {
        return match lang {
            Lang::En => "Not linked".into(),
            Lang::Ru => "Не подключено".into(),
        };
    }
    if paused {
        return match lang {
            Lang::En => "Paused".into(),
            Lang::Ru => "На паузе".into(),
        };
    }
    match (lang, state) {
        (Lang::En, LinkState::Linked) => format!("Linked to {synth}"),
        (Lang::Ru, LinkState::Linked) => format!("Подключено: {synth}"),
        (Lang::En, _) => format!("Connecting to {synth}..."),
        (Lang::Ru, _) => format!("Подключение: {synth}..."),
    }
}

pub fn tray_pause(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Pause",
        Lang::Ru => "Пауза",
    }
}

pub fn tray_resume(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Resume",
        Lang::Ru => "Продолжить",
    }
}

pub fn tray_open(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Open SNTH Connect",
        Lang::Ru => "Открыть SNTH Connect",
    }
}

pub fn tray_quit(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Quit",
        Lang::Ru => "Выйти",
    }
}

pub fn paused_title(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "SNTH Connect is paused",
        Lang::Ru => "SNTH Connect на паузе",
    }
}

pub fn paused_body(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Your synth can do nothing on this computer until you resume.",
        Lang::Ru => "Синт ничего не может делать на этом компьютере, пока вы не продолжите.",
    }
}

pub fn approval_title(lang: Lang, synth: &str) -> String {
    match lang {
        Lang::En => format!("{synth} asks for permission"),
        Lang::Ru => format!("{synth} просит разрешение"),
    }
}

/// The title when it is not the synth that asks but the owner's own
/// browser (its SNTH Vault extension wants to be linked).
pub fn browser_asks_title(lang: Lang, browser: &str) -> String {
    match lang {
        Lang::En => format!("{browser} asks to be linked"),
        Lang::Ru => format!("{browser} просит привязку"),
    }
}

/// One plain line for the notification. The approval window has the full
/// wording; this only says what kind of request is waiting.
pub fn approval_body(lang: Lang, capability: &str, scope: &str) -> String {
    let file_name = || {
        scope
            .rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or(scope)
            .to_string()
    };
    match (lang, capability) {
        (Lang::En, "net") => format!("Use this computer's internet to open {scope}"),
        (Lang::Ru, "net") => format!("Открыть {scope} через интернет этого компьютера"),
        (Lang::En, "cookies") => format!("Use your sign-in on {scope}"),
        (Lang::Ru, "cookies") => format!("Использовать ваш вход на {scope}"),
        (Lang::En, "computer") if scope.starts_with("foreground:") => {
            "Take over the screen for a moment".into()
        }
        (Lang::Ru, "computer") if scope.starts_with("foreground:") => {
            "Ненадолго занять экран".into()
        }
        (Lang::En, "computer") => "Control this computer for a task".into(),
        (Lang::Ru, "computer") => "Управлять этим компьютером для задачи".into(),
        (Lang::En, "files") => format!("Change a file: {}", file_name()),
        (Lang::Ru, "files") => format!("Изменить файл: {}", file_name()),
        // Only the kind of action: what a message says is shown in the
        // request window, not in a notification on the lock screen.
        (Lang::En, "apps") if scope.starts_with("messages.send:") => "Send a message as you".into(),
        (Lang::Ru, "apps") if scope.starts_with("messages.send:") => "Отправить сообщение от вашего имени".into(),
        (Lang::En, "apps") => "Change something in your apps".into(),
        (Lang::Ru, "apps") => "Изменить что-то в ваших приложениях".into(),
        // A command: never the command itself on a lock screen.
        (Lang::En, "shell") => "Run a command on this computer".into(),
        (Lang::Ru, "shell") => "Выполнить команду на этом компьютере".into(),
        // The SNTH Vault extension in a browser on this computer.
        (Lang::En, "vault") => "Share site passwords with your synth from this browser".into(),
        (Lang::Ru, "vault") => "Передавать синту пароли сайтов из этого браузера".into(),
        (Lang::En, _) => "Open the request window to answer.".into(),
        (Lang::Ru, _) => "Откройте окно запроса, чтобы ответить.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A notification can show on a locked screen: for a change in the
    /// owner's apps it says what kind of request waits, never who a
    /// message is for or anything else from the request.
    #[test]
    fn an_apps_notification_names_no_recipient() {
        for lang in [Lang::En, Lang::Ru] {
            let send = approval_body(lang, "apps", "messages.send:+79161234567");
            assert!(!send.contains("7916") && !send.contains("messages.send"), "{send}");
            let other = approval_body(lang, "apps", "shortcuts.run:Unlock Front Door");
            assert!(!other.contains("Unlock") && !other.is_empty(), "{other}");
            assert_ne!(send, other);
            let cmd = approval_body(lang, "shell", "rm -rf ~/Secret");
            assert!(!cmd.contains("Secret") && !cmd.is_empty(), "{cmd}");
        }
    }
}
