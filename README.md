# shell (рабочее название)

Оболочка рабочего стола для X11 в стиле macOS, на Rust: строка меню, док,
Launchpad, Spotlight, уведомления. Окна рисует обычный оконный менеджер (xfwm4).

## Принцип
Изучаем, **как ведут себя** macOS и существующие программы (панель Xfce, Plank,
rofi, vala-panel-appmenu) и **открытые стандарты** (EWMH/ICCCM, freedesktop:
Desktop Entry, Icon Theme, StatusNotifierItem, DBusMenu, Notifications) —
и пишем **свою реализацию со своей логикой**. Чужой код не копируем.

## Слои (зависимости только сверху вниз)
- `core` — конфиг и модель, без окон и ввода-вывода
- `x11`, `render`, `text`, `icons`, `apps`, `dbus` — платформа (появятся по мере работы)
- модули оболочки: `launcher`, `dock`, `menubar`, `notify`, `lock` (сторож для i3lock-color)
- `apps/shell` — сама программа

Блокировку экрана сами не пишем: только запускаем и сторожим проверенный i3lock-color.
