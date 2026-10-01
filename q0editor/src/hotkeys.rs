//! User-configurable keyboard shortcuts shared by menus and input handling.

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutCommand {
    PerformanceSnapshot,
    NextTab,
    PreviousTab,
    NewProject,
    OpenProject,
    SaveProject,
    SaveProjectAs,
    Export,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    PasteInPlace,
    Duplicate,
    Delete,
    ConvertToSymbol,
    BreakApart,
    FlipHorizontal,
    FlipVertical,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ScriptEditor,
    PlaybackToggle,
    ToggleTimeline,
    FirstFrame,
    LastFrame,
    PreviousFrame,
    NextFrame,
    InsertFrame,
    RemoveFrame,
    InsertKeyframe,
    ClearKeyframe,
    InsertBlankKeyframe,
    MotionTween,
    ClearSelection,
    ToolSelect,
    ToolLasso,
    ToolSubselect,
    ToolHand,
    ToolPen,
    ToolPencil,
    ToolBrush,
    ToolEraser,
    ToolLine,
    ToolRectangle,
    ToolOval,
    ToolBucket,
    ToolEyedropper,
    ToolText,
}

impl ShortcutCommand {
    pub const ALL: &'static [Self] = &[
        Self::PerformanceSnapshot,
        Self::NextTab,
        Self::PreviousTab,
        Self::NewProject,
        Self::OpenProject,
        Self::SaveProject,
        Self::SaveProjectAs,
        Self::Export,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::PasteInPlace,
        Self::Duplicate,
        Self::Delete,
        Self::ConvertToSymbol,
        Self::BreakApart,
        Self::FlipHorizontal,
        Self::FlipVertical,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomReset,
        Self::ScriptEditor,
        Self::PlaybackToggle,
        Self::ToggleTimeline,
        Self::FirstFrame,
        Self::LastFrame,
        Self::PreviousFrame,
        Self::NextFrame,
        Self::InsertFrame,
        Self::RemoveFrame,
        Self::InsertKeyframe,
        Self::ClearKeyframe,
        Self::InsertBlankKeyframe,
        Self::MotionTween,
        Self::ClearSelection,
        Self::ToolSelect,
        Self::ToolLasso,
        Self::ToolSubselect,
        Self::ToolHand,
        Self::ToolPen,
        Self::ToolPencil,
        Self::ToolBrush,
        Self::ToolEraser,
        Self::ToolLine,
        Self::ToolRectangle,
        Self::ToolOval,
        Self::ToolBucket,
        Self::ToolEyedropper,
        Self::ToolText,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::PerformanceSnapshot => "Copy performance snapshot",
            Self::NextTab => "Next document tab",
            Self::PreviousTab => "Previous document tab",
            Self::NewProject => "New project",
            Self::OpenProject => "Open project",
            Self::SaveProject => "Save project",
            Self::SaveProjectAs => "Save project as",
            Self::Export => "Export",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut selection",
            Self::Copy => "Copy selection",
            Self::Paste => "Paste",
            Self::PasteInPlace => "Paste in place",
            Self::Duplicate => "Duplicate selection",
            Self::Delete => "Delete selection",
            Self::ConvertToSymbol => "Convert to symbol",
            Self::BreakApart => "Break apart",
            Self::FlipHorizontal => "flip horizontally",
            Self::FlipVertical => "flip vertically",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::ZoomReset => "Reset zoom",
            Self::ScriptEditor => "Open q0lang editor",
            Self::PlaybackToggle => "Play / stop",
            Self::ToggleTimeline => "Show or hide timeline",
            Self::FirstFrame => "Go to first frame",
            Self::LastFrame => "Go to last frame",
            Self::PreviousFrame => "Previous frame",
            Self::NextFrame => "Next frame",
            Self::InsertFrame => "Insert frame",
            Self::RemoveFrame => "Remove selected frames",
            Self::InsertKeyframe => "Insert keyframe",
            Self::ClearKeyframe => "Clear keyframe",
            Self::InsertBlankKeyframe => "Insert blank keyframe",
            Self::MotionTween => "Create motion tween",
            Self::ClearSelection => "Clear selection",
            Self::ToolSelect => "Select tool",
            Self::ToolLasso => "Lasso tool",
            Self::ToolSubselect => "Subselect tool",
            Self::ToolHand => "Hand tool",
            Self::ToolPen => "Pen tool",
            Self::ToolPencil => "Pencil tool",
            Self::ToolBrush => "Brush tool",
            Self::ToolEraser => "Eraser tool",
            Self::ToolLine => "Line tool",
            Self::ToolRectangle => "Rectangle tool",
            Self::ToolOval => "Oval tool",
            Self::ToolBucket => "Bucket tool",
            Self::ToolEyedropper => "Eyedropper tool",
            Self::ToolText => "Text tool",
        }
    }

    pub fn default_shortcuts(self) -> Vec<ShortcutChord> {
        use ShortcutCommand as C;
        use ShortcutKey as K;
        match self {
            C::PerformanceSnapshot => vec![ShortcutChord::key(K::F4)],
            C::NextTab => vec![ShortcutChord::cmd(K::Tab)],
            C::PreviousTab => vec![ShortcutChord::cmd_shift(K::Tab)],
            C::NewProject => vec![ShortcutChord::cmd(K::N)],
            C::OpenProject => vec![ShortcutChord::cmd(K::O)],
            C::SaveProject => vec![ShortcutChord::cmd(K::S)],
            C::SaveProjectAs => vec![ShortcutChord::cmd_shift(K::S)],
            C::Export => vec![ShortcutChord::cmd(K::E)],
            C::Undo => vec![ShortcutChord::cmd(K::Z)],
            C::Redo => vec![ShortcutChord::cmd_shift(K::Z), ShortcutChord::cmd(K::Y)],
            C::Cut => vec![ShortcutChord::cmd(K::X)],
            C::Copy => vec![ShortcutChord::cmd(K::C)],
            C::Paste => vec![ShortcutChord::cmd(K::V)],
            C::PasteInPlace => vec![ShortcutChord::cmd_shift(K::V)],
            C::Duplicate => vec![ShortcutChord::cmd(K::D)],
            C::Delete => vec![
                ShortcutChord::key(K::Delete),
                ShortcutChord::key(K::Backspace),
            ],
            C::ConvertToSymbol => vec![ShortcutChord::cmd(K::G)],
            C::BreakApart => vec![ShortcutChord::cmd(K::B)],
            C::FlipHorizontal => vec![ShortcutChord::shift(K::H)],
            C::FlipVertical => vec![ShortcutChord::shift(K::V)],
            C::ZoomIn => vec![ShortcutChord::cmd(K::Plus), ShortcutChord::cmd(K::Equals)],
            C::ZoomOut => vec![ShortcutChord::cmd(K::Minus)],
            C::ZoomReset => vec![ShortcutChord::cmd(K::Num0)],
            C::ScriptEditor => vec![ShortcutChord::key(K::F9)],
            C::PlaybackToggle => vec![ShortcutChord::key(K::Enter)],
            C::ToggleTimeline => vec![ShortcutChord::cmd_alt(K::T)],
            C::FirstFrame => vec![ShortcutChord::key(K::Home)],
            C::LastFrame => vec![ShortcutChord::key(K::End)],
            C::PreviousFrame => vec![ShortcutChord::key(K::ArrowLeft)],
            C::NextFrame => vec![ShortcutChord::key(K::ArrowRight)],
            C::InsertFrame => vec![ShortcutChord::key(K::F5)],
            C::RemoveFrame => vec![ShortcutChord::shift(K::F5)],
            C::InsertKeyframe => vec![ShortcutChord::key(K::F6)],
            C::ClearKeyframe => vec![ShortcutChord::shift(K::F6)],
            C::InsertBlankKeyframe => vec![ShortcutChord::key(K::F7)],
            C::MotionTween => vec![ShortcutChord::cmd_alt(K::M)],
            C::ClearSelection => vec![ShortcutChord::key(K::Escape)],
            C::ToolSelect => vec![ShortcutChord::key(K::V)],
            C::ToolLasso => vec![ShortcutChord::key(K::Q)],
            C::ToolSubselect => vec![ShortcutChord::key(K::A)],
            C::ToolHand => vec![ShortcutChord::key(K::H)],
            C::ToolPen => vec![ShortcutChord::key(K::P)],
            C::ToolPencil => vec![ShortcutChord::key(K::Y)],
            C::ToolBrush => vec![ShortcutChord::key(K::B)],
            C::ToolEraser => vec![ShortcutChord::key(K::E)],
            C::ToolLine => vec![ShortcutChord::key(K::N)],
            C::ToolRectangle => vec![ShortcutChord::key(K::R)],
            C::ToolOval => vec![ShortcutChord::key(K::O)],
            C::ToolBucket => vec![ShortcutChord::key(K::K)],
            C::ToolEyedropper => vec![ShortcutChord::key(K::I)],
            C::ToolText => vec![ShortcutChord::key(K::T)],
        }
    }

    pub const fn clipboard_key(self) -> Option<Key> {
        match self {
            Self::Cut => Some(Key::X),
            Self::Copy => Some(Key::C),
            Self::Paste | Self::PasteInPlace => Some(Key::V),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutKey {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Escape,
    Tab,
    Backspace,
    Enter,
    Space,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Minus,
    Plus,
    Equals,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl ShortcutKey {
    pub fn from_egui(key: Key) -> Option<Self> {
        Some(match key {
            Key::ArrowDown => Self::ArrowDown,
            Key::ArrowLeft => Self::ArrowLeft,
            Key::ArrowRight => Self::ArrowRight,
            Key::ArrowUp => Self::ArrowUp,
            Key::Escape => Self::Escape,
            Key::Tab => Self::Tab,
            Key::Backspace => Self::Backspace,
            Key::Enter => Self::Enter,
            Key::Space => Self::Space,
            Key::Insert => Self::Insert,
            Key::Delete => Self::Delete,
            Key::Home => Self::Home,
            Key::End => Self::End,
            Key::PageUp => Self::PageUp,
            Key::PageDown => Self::PageDown,
            Key::Minus => Self::Minus,
            Key::Plus => Self::Plus,
            Key::Equals => Self::Equals,
            Key::Num0 => Self::Num0,
            Key::Num1 => Self::Num1,
            Key::Num2 => Self::Num2,
            Key::Num3 => Self::Num3,
            Key::Num4 => Self::Num4,
            Key::Num5 => Self::Num5,
            Key::Num6 => Self::Num6,
            Key::Num7 => Self::Num7,
            Key::Num8 => Self::Num8,
            Key::Num9 => Self::Num9,
            Key::A => Self::A,
            Key::B => Self::B,
            Key::C => Self::C,
            Key::D => Self::D,
            Key::E => Self::E,
            Key::F => Self::F,
            Key::G => Self::G,
            Key::H => Self::H,
            Key::I => Self::I,
            Key::J => Self::J,
            Key::K => Self::K,
            Key::L => Self::L,
            Key::M => Self::M,
            Key::N => Self::N,
            Key::O => Self::O,
            Key::P => Self::P,
            Key::Q => Self::Q,
            Key::R => Self::R,
            Key::S => Self::S,
            Key::T => Self::T,
            Key::U => Self::U,
            Key::V => Self::V,
            Key::W => Self::W,
            Key::X => Self::X,
            Key::Y => Self::Y,
            Key::Z => Self::Z,
            Key::F1 => Self::F1,
            Key::F2 => Self::F2,
            Key::F3 => Self::F3,
            Key::F4 => Self::F4,
            Key::F5 => Self::F5,
            Key::F6 => Self::F6,
            Key::F7 => Self::F7,
            Key::F8 => Self::F8,
            Key::F9 => Self::F9,
            Key::F10 => Self::F10,
            Key::F11 => Self::F11,
            Key::F12 => Self::F12,
            _ => return None,
        })
    }

    pub const fn to_egui(self) -> Key {
        match self {
            Self::ArrowDown => Key::ArrowDown,
            Self::ArrowLeft => Key::ArrowLeft,
            Self::ArrowRight => Key::ArrowRight,
            Self::ArrowUp => Key::ArrowUp,
            Self::Escape => Key::Escape,
            Self::Tab => Key::Tab,
            Self::Backspace => Key::Backspace,
            Self::Enter => Key::Enter,
            Self::Space => Key::Space,
            Self::Insert => Key::Insert,
            Self::Delete => Key::Delete,
            Self::Home => Key::Home,
            Self::End => Key::End,
            Self::PageUp => Key::PageUp,
            Self::PageDown => Key::PageDown,
            Self::Minus => Key::Minus,
            Self::Plus => Key::Plus,
            Self::Equals => Key::Equals,
            Self::Num0 => Key::Num0,
            Self::Num1 => Key::Num1,
            Self::Num2 => Key::Num2,
            Self::Num3 => Key::Num3,
            Self::Num4 => Key::Num4,
            Self::Num5 => Key::Num5,
            Self::Num6 => Key::Num6,
            Self::Num7 => Key::Num7,
            Self::Num8 => Key::Num8,
            Self::Num9 => Key::Num9,
            Self::A => Key::A,
            Self::B => Key::B,
            Self::C => Key::C,
            Self::D => Key::D,
            Self::E => Key::E,
            Self::F => Key::F,
            Self::G => Key::G,
            Self::H => Key::H,
            Self::I => Key::I,
            Self::J => Key::J,
            Self::K => Key::K,
            Self::L => Key::L,
            Self::M => Key::M,
            Self::N => Key::N,
            Self::O => Key::O,
            Self::P => Key::P,
            Self::Q => Key::Q,
            Self::R => Key::R,
            Self::S => Key::S,
            Self::T => Key::T,
            Self::U => Key::U,
            Self::V => Key::V,
            Self::W => Key::W,
            Self::X => Key::X,
            Self::Y => Key::Y,
            Self::Z => Key::Z,
            Self::F1 => Key::F1,
            Self::F2 => Key::F2,
            Self::F3 => Key::F3,
            Self::F4 => Key::F4,
            Self::F5 => Key::F5,
            Self::F6 => Key::F6,
            Self::F7 => Key::F7,
            Self::F8 => Key::F8,
            Self::F9 => Key::F9,
            Self::F10 => Key::F10,
            Self::F11 => Key::F11,
            Self::F12 => Key::F12,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ArrowDown => "Down",
            Self::ArrowLeft => "Left",
            Self::ArrowRight => "Right",
            Self::ArrowUp => "Up",
            Self::Escape => "Esc",
            Self::Tab => "Tab",
            Self::Backspace => "Backspace",
            Self::Enter => "Enter",
            Self::Space => "Space",
            Self::Insert => "Insert",
            Self::Delete => "Del",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "Page Up",
            Self::PageDown => "Page Down",
            Self::Minus => "-",
            Self::Plus => "+",
            Self::Equals => "=",
            Self::Num0 => "0",
            Self::Num1 => "1",
            Self::Num2 => "2",
            Self::Num3 => "3",
            Self::Num4 => "4",
            Self::Num5 => "5",
            Self::Num6 => "6",
            Self::Num7 => "7",
            Self::Num8 => "8",
            Self::Num9 => "9",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShortcutChord {
    pub key: ShortcutKey,
    #[serde(default)]
    pub command: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
}

impl ShortcutChord {
    pub const fn key(key: ShortcutKey) -> Self {
        Self {
            key,
            command: false,
            shift: false,
            alt: false,
        }
    }
    pub const fn cmd(key: ShortcutKey) -> Self {
        Self {
            key,
            command: true,
            shift: false,
            alt: false,
        }
    }
    pub const fn shift(key: ShortcutKey) -> Self {
        Self {
            key,
            command: false,
            shift: true,
            alt: false,
        }
    }
    pub const fn cmd_shift(key: ShortcutKey) -> Self {
        Self {
            key,
            command: true,
            shift: true,
            alt: false,
        }
    }
    pub const fn cmd_alt(key: ShortcutKey) -> Self {
        Self {
            key,
            command: true,
            shift: false,
            alt: true,
        }
    }

    pub fn from_modifiers(key: Key, modifiers: Modifiers) -> Option<Self> {
        let key = ShortcutKey::from_egui(key)?;
        Some(Self {
            key,
            command: modifiers.command || modifiers.ctrl || modifiers.mac_cmd,
            shift: modifiers.shift,
            alt: modifiers.alt,
        })
    }

    pub fn modifiers(self) -> Modifiers {
        let mut modifiers = if self.command {
            Modifiers::COMMAND
        } else {
            Modifiers::NONE
        };
        if self.shift {
            modifiers = modifiers | Modifiers::SHIFT;
        }
        if self.alt {
            modifiers = modifiers | Modifiers::ALT;
        }
        modifiers
    }

    pub fn matches(self, input: &mut egui::InputState) -> bool {
        input.consume_key(self.modifiers(), self.key.to_egui())
    }

    pub fn display(self) -> String {
        let mut parts = Vec::with_capacity(4);
        if self.command {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        parts.push(self.key.label().to_string());
        parts.join("+")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutOverride {
    pub command: ShortcutCommand,
    pub shortcuts: Vec<ShortcutChord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub overrides: Vec<ShortcutOverride>,
}

impl HotkeySettings {
    pub fn hint(&self, command: ShortcutCommand, label: &str) -> String {
        if self.shortcuts(command).is_empty() {
            label.to_string()
        } else {
            format!("{label} ({})", self.display(command))
        }
    }

    pub fn shortcuts(&self, command: ShortcutCommand) -> Vec<ShortcutChord> {
        if let Some(binding) = self
            .overrides
            .iter()
            .find(|binding| binding.command == command)
        {
            binding.shortcuts.clone()
        } else {
            command.default_shortcuts()
        }
    }

    pub fn display(&self, command: ShortcutCommand) -> String {
        let shortcuts = self.shortcuts(command);
        if shortcuts.is_empty() {
            "Unassigned".to_string()
        } else {
            shortcuts
                .into_iter()
                .map(ShortcutChord::display)
                .collect::<Vec<_>>()
                .join(" / ")
        }
    }

    pub fn command_for(&self, chord: ShortcutChord) -> Option<ShortcutCommand> {
        ShortcutCommand::ALL
            .iter()
            .copied()
            .find(|command| self.shortcuts(*command).contains(&chord))
    }

    /// Assigning a chord moves it away from its previous command.
    pub fn assign(
        &mut self,
        command: ShortcutCommand,
        shortcut: ShortcutChord,
    ) -> Vec<ShortcutCommand> {
        let moved = ShortcutCommand::ALL
            .iter()
            .copied()
            .filter(|other| *other != command && self.shortcuts(*other).contains(&shortcut))
            .collect::<Vec<_>>();
        for other in &moved {
            let mut shortcuts = self.shortcuts(*other);
            shortcuts.retain(|existing| *existing != shortcut);
            self.set(*other, shortcuts);
        }
        self.set(command, vec![shortcut]);
        moved
    }

    pub fn set(&mut self, command: ShortcutCommand, shortcuts: Vec<ShortcutChord>) {
        if let Some(binding) = self
            .overrides
            .iter_mut()
            .find(|binding| binding.command == command)
        {
            binding.shortcuts = shortcuts;
        } else {
            self.overrides.push(ShortcutOverride { command, shortcuts });
        }
    }

    pub fn reset(&mut self) {
        self.overrides.clear();
    }

    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.overrides
            .retain(|binding| seen.insert(binding.command));
    }
}
