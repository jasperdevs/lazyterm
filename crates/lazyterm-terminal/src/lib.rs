#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub const DEFAULT: Self = Self {
        columns: 120,
        rows: 34,
    };

    pub const fn new(columns: u16, rows: u16) -> Self {
        Self { columns, rows }
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSurface {
    pub size: TerminalSize,
    pub title: String,
}

impl TerminalSurface {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            size: TerminalSize::default(),
            title: title.into(),
        }
    }

    pub fn resize(&mut self, size: TerminalSize) {
        self.size = size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_surface_uses_default_size() {
        let surface = TerminalSurface::new("shell");

        assert_eq!(surface.title, "shell");
        assert_eq!(surface.size, TerminalSize::DEFAULT);
    }

    #[test]
    fn resizes_surface() {
        let mut surface = TerminalSurface::new("shell");
        surface.resize(TerminalSize::new(80, 24));

        assert_eq!(surface.size, TerminalSize::new(80, 24));
    }
}
