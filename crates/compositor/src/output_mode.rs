//! Temporary output modes and bounded session control.

/// A validated temporary mode request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Set {
        width: u16,
        height: u16,
        refresh: Option<u32>,
    },
    Restore,
}

impl Request {
    /// Parses the command argument. Refresh rates are whole Hz.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value == "--restore" {
            return Some(Self::Restore);
        }
        let (size, refresh) = match value.split_once('@') {
            Some((size, refresh)) => (size, Some(refresh.parse::<u32>().ok()?)),
            None => (value, None),
        };
        let (width, height) = size.split_once('x')?;
        let (width, height) = (width.parse::<u16>().ok()?, height.parse::<u16>().ok()?);
        (width > 0 && height > 0 && refresh != Some(0)).then_some(Self::Set {
            width,
            height,
            refresh,
        })
    }

    /// Chooses the closest dimensions, then refresh rate; ties keep driver order.
    #[must_use]
    pub fn closest(self, modes: &[(u16, u16, u32)]) -> Option<usize> {
        let Self::Set {
            width,
            height,
            refresh,
        } = self
        else {
            return None;
        };
        modes
            .iter()
            .enumerate()
            .min_by_key(|(_, (candidate_width, candidate_height, candidate_refresh))| {
                (
                    u32::from(candidate_width.abs_diff(width))
                        .saturating_add(u32::from(candidate_height.abs_diff(height))),
                    refresh.map_or(0, |refresh| candidate_refresh.abs_diff(refresh)),
                )
            })
            .map(|(index, _)| index)
    }
}

/// The first mode displaced by a temporary streaming override.
#[derive(Debug)]
pub struct Temporary<Mode> {
    saved: Option<Mode>,
}

impl<Mode> Default for Temporary<Mode> {
    fn default() -> Self {
        Self { saved: None }
    }
}

impl<Mode: Copy + PartialEq> Temporary<Mode> {
    /// Applies a temporary mode, or restores when `requested` is `None`.
    ///
    /// # Errors
    /// A rejected switch leaves the restore point unchanged.
    pub fn apply<Error>(
        &mut self,
        current: Mode,
        requested: Option<Mode>,
        switch: impl FnOnce(Mode) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let Some(target) = requested.or(self.saved) else {
            return Ok(());
        };
        if target != current {
            switch(target)?;
        }
        if requested.is_some() {
            let _saved = self.saved.get_or_insert(current);
        } else {
            self.saved = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_supported_dimensions_precede_refresh_and_refresh_is_optional() {
        let modes = [(1920, 1080, 60), (1280, 720, 60), (1280, 720, 120)];
        assert_eq!(
            Request::parse("1280x720@100").and_then(|request| request.closest(&modes)),
            Some(2)
        );
        assert_eq!(
            Request::parse("1300x740@60").and_then(|request| request.closest(&modes)),
            Some(1)
        );
        assert_eq!(
            Request::parse("1280x720").and_then(|request| request.closest(&modes)),
            Some(1)
        );
        assert_eq!(
            Request::parse("1280x720@60").and_then(|request| request.closest(&[])),
            None
        );
        for value in [
            "0x720",
            "1280x0",
            "1280x720@0",
            "65536x720",
            "1280x720@60junk",
            "1280x720@-1",
        ] {
            assert_eq!(Request::parse(value), None);
        }
    }

    #[test]
    fn repeated_requests_restore_the_original_mode_and_restore_is_idempotent() {
        let mut session = Temporary::default();
        let mut current = 1920;
        for requested in [Some(1280), Some(2560), None, None] {
            session
                .apply(current, requested, |target| {
                    current = target;
                    Ok::<_, ()>(())
                })
                .unwrap();
        }
        assert_eq!(current, 1920);
        session
            .apply(current, Some(3840), |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        session
            .apply(current, None, |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(current, 1920);
    }

    #[test]
    fn failed_switches_preserve_the_restore_point() {
        let mut session = Temporary::default();
        assert_eq!(
            session.apply(1920, Some(1280), |_| Err("rejected")),
            Err("rejected")
        );
        let mut current = 2560;
        session
            .apply(current, None, |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(current, 2560);
        session
            .apply(current, Some(1280), |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(session.apply(current, None, |_| Err("rejected")), Err("rejected"));
        session
            .apply(current, None, |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(current, 2560);
    }
}
