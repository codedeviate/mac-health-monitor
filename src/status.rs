/// Health of one monitored resource. Ordered `Ok < Warning < Critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Status {
    #[default]
    Ok,
    Warning,
    Critical,
}

/// Kernel memory pressure level, as reported by `sysctl kern.memorystatus_vm_pressure_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PressureLevel {
    #[default]
    Normal,
    Warning,
    Critical,
}

impl PressureLevel {
    /// Maps the raw kernel value: 2 is Warning, 4 is Critical, anything else is Normal.
    pub fn from_raw(v: i32) -> PressureLevel {
        match v {
            2 => PressureLevel::Warning,
            4 => PressureLevel::Critical,
            _ => PressureLevel::Normal,
        }
    }

    pub fn status(self) -> Status {
        match self {
            PressureLevel::Normal => Status::Ok,
            PressureLevel::Warning => Status::Warning,
            PressureLevel::Critical => Status::Critical,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PressureLevel::Normal => "Normal",
            PressureLevel::Warning => "Warning",
            PressureLevel::Critical => "Critical",
        }
    }
}

/// Icon shown in the menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    ThumbsUp,
    Chip,
    Drive,
    Recycle,
    Triangle,
}

/// Current status of each monitored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overall {
    pub disk: Status,
    pub memory: Status,
    pub swap: Status,
}

/// Picks the menu-bar icon and its color.
///
/// All OK gives the thumbs-up. A single issue gives that resource's icon in its
/// status color. Multiple issues give the triangle in the worst status color.
pub fn select_icon(o: &Overall) -> (IconKind, Status) {
    let worst = o.disk.max(o.memory).max(o.swap);
    let flagged: Vec<(Status, IconKind)> = [
        (o.disk, IconKind::Drive),
        (o.memory, IconKind::Chip),
        (o.swap, IconKind::Recycle),
    ]
    .into_iter()
    .filter(|(s, _)| *s != Status::Ok)
    .collect();

    match flagged.as_slice() {
        [] => (IconKind::ThumbsUp, Status::Ok),
        [(status, icon)] => (*icon, *status),
        _ => (IconKind::Triangle, worst),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::*;

    fn ov(d: Status, m: Status, s: Status) -> Overall {
        Overall {
            disk: d,
            memory: m,
            swap: s,
        }
    }

    #[test]
    fn all_ok_is_green_thumb() {
        assert_eq!(
            select_icon(&ov(Ok, Ok, Ok)),
            (IconKind::ThumbsUp, Status::Ok)
        );
    }

    #[test]
    fn single_issue_picks_its_icon() {
        assert_eq!(select_icon(&ov(Ok, Warning, Ok)), (IconKind::Chip, Warning));
        assert_eq!(
            select_icon(&ov(Critical, Ok, Ok)),
            (IconKind::Drive, Critical)
        );
        assert_eq!(
            select_icon(&ov(Ok, Ok, Warning)),
            (IconKind::Recycle, Warning)
        );
    }

    #[test]
    fn multiple_issues_triangle_worst_color() {
        assert_eq!(
            select_icon(&ov(Warning, Ok, Critical)),
            (IconKind::Triangle, Critical)
        );
        assert_eq!(
            select_icon(&ov(Warning, Warning, Ok)),
            (IconKind::Triangle, Warning)
        );
    }

    #[test]
    fn pressure_from_raw() {
        assert_eq!(PressureLevel::from_raw(1), PressureLevel::Normal);
        assert_eq!(PressureLevel::from_raw(2), PressureLevel::Warning);
        assert_eq!(PressureLevel::from_raw(4), PressureLevel::Critical);
        assert_eq!(PressureLevel::from_raw(0), PressureLevel::Normal);
        assert_eq!(PressureLevel::from_raw(99), PressureLevel::Normal);
    }
}
