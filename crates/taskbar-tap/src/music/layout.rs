//! The tile's geometry and XAML, all derived from one number: the strip width.
//!
//! The constants were measured on a real taskbar (see FINDINGS.md, "The music tile"). The
//! geometry is a pure function of the width, so it is unit-tested here.

/// The width the strip lays itself out in. One width whatever is showing, so the button never
/// resizes under the pointer (sized so `Nothing playing` fits without scrolling).
///
/// ```text
/// 2·pad 4  +  cover 28  +  gap 6  +  text 108  +  slack 4  =  150
/// ```
pub const STRIP_WIDTH: u32 = 150;

// The extra width the button must be asked for lives on `super::tile::Host::SLOT_OVERHEAD`.

/// Font sizes, in effective pixels. The column is fixed width; the ticker scrolls overflow.
pub const TITLE_SIZE: u32 = 14;
pub const ARTIST_SIZE: u32 = 11;

/// The text column's height and the title's line box: 17 + the artist's natural 14.6 fits 32
/// without clipping descenders. `BlockLineHeight` is what makes `LineHeight` binding.
const TEXT_HEIGHT: u32 = 32;
const TITLE_LINE: u32 = 17;

/// Average epx per character × 100 at [`TITLE_SIZE`] / [`ARTIST_SIZE`], measured off rendered
/// text. Unusually wide titles overflow by a character into the `Clip`, the lesser error.
const TITLE_EPX_PER_CHAR: u32 = 649;
const ARTIST_EPX_PER_CHAR: u32 = 528;

/// The strip's internal geometry, derived from the width it has to fill.
pub struct Layout {
    /// Total content width — what the root plate is set to.
    pub strip: u32,
    pub pad: u32,
    pub cover: u32,
    /// Between the cover and the text.
    pub gap: u32,
    pub text: u32,
    pub title_chars: usize,
    pub artist_chars: usize,
}

impl Layout {
    /// Fit the strip's parts into `strip` epx: fixed parts sized by the room available, the rest
    /// to the text column (the one part that degrades gracefully, by scrolling).
    pub fn for_width(strip: u32) -> Self {
        // The shipped width must take the roomy branch (tested).
        let roomy = strip >= 122;
        let pad = if roomy { 2 } else { 1 };
        let cover = if roomy { 28 } else { 26 };
        let gap = if roomy { 6 } else { 3 };

        // Load-bearing: room for the last character's ink overshoot at the plate's edge.
        const SLACK: u32 = 4;
        let fixed = 2 * pad + cover + gap + SLACK;
        let text = strip.saturating_sub(fixed);

        Self {
            strip,
            pad,
            cover,
            gap,
            text,
            // Rounded, not truncated, so an exact fit is not scrolled.
            title_chars: ((text * 100 + TITLE_EPX_PER_CHAR / 2) / TITLE_EPX_PER_CHAR) as usize,
            artist_chars: ((text * 100 + ARTIST_EPX_PER_CHAR / 2) / ARTIST_EPX_PER_CHAR) as usize,
        }
    }
}

/// The width the strip lays its content out in: [`STRIP_WIDTH`] unless the init data passed
/// `strip=<epx>`. The button is asked for this plus `Host::SLOT_OVERHEAD`.
static CONTENT_WIDTH: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(STRIP_WIDTH);

pub fn set_content_width(width: u32) {
    CONTENT_WIDTH.store(width, std::sync::atomic::Ordering::SeqCst);
}

/// The live layout; cheap, so recomputed per use rather than cached.
pub fn layout() -> Layout {
    Layout::for_width(CONTENT_WIDTH.load(std::sync::atomic::Ordering::SeqCst))
}

/// `x:Name` of the `Border` holding the cover art, so a track change can swap the art alone.
pub const COVER_HOST: &str = "MusicTileCoverHost";

/// The cover art and its note-glyph placeholder, the only part a track change rebuilds. Both are
/// always present (`Visibility` picks one), so the subtree's shape never changes. Rebuilt rather
/// than property-set because a new `Image.Source` needs a fresh `BitmapImage` (no binding here,
/// and it caches by URI).
pub fn cover_markup(strip: &super::state::Strip, cover_px: u32, gap: u32) -> String {
    use super::state::escape;

    format!(
        r#"<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
                 xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
                 Width="{cover_px}" Height="{cover_px}" Margin="0,0,{gap},0">
             <Border x:Name="MusicTileCoverPlaceholder" CornerRadius="2"
                     Background="{{ThemeResource SystemControlBackgroundBaseLowBrush}}"
                     Visibility="{placeholder}">
               <TextBlock Text="&#xE8D6;" FontFamily="Segoe Fluent Icons" FontSize="12"
                          HorizontalAlignment="Center" VerticalAlignment="Center"
                          Foreground="{{ThemeResource SystemControlForegroundBaseMediumBrush}}"/>
             </Border>
             <Border CornerRadius="2">
               <Image x:Name="MusicTileCover" Stretch="UniformToFill" Visibility="{art}">{source}</Image>
             </Border>
           </Grid>"#,
        art = if strip.cover.is_some() { "Visible" } else { "Collapsed" },
        placeholder = if strip.cover.is_some() {
            "Collapsed"
        } else {
            "Visible"
        },
        source = match strip.cover.as_deref() {
            Some(path) => format!(
                r#"<Image.Source><BitmapImage UriSource="file:///{}"/></Image.Source>"#,
                escape(&path.replace('\\', "/"))
            ),
            None => String::new(),
        },
    )
}

pub fn now_playing_markup(strip: &super::state::Strip) -> String {
    use super::state::escape;

    let l = layout();

    // Own `Border` so a track change replaces just this (`super::tile::update_cover`).
    let cover = format!(
        r#"<Border x:Name="{COVER_HOST}">{}</Border>"#,
        cover_markup(strip, l.cover, l.gap)
    );

    // Fixed width (not `MaxWidth`) so the layout never moves with the content. The `Clip` is
    // needed because XAML panels do not clip children; overflow is scrolled by `super::ticker`.
    let text = format!(
        r#"<Border Width="{text_px}" Height="{TEXT_HEIGHT}" Margin="0,0,2,0" Background="Transparent">
             <Border.Clip>
               <RectangleGeometry Rect="0,0,{text_px},{TEXT_HEIGHT}"/>
             </Border.Clip>
             <StackPanel VerticalAlignment="Center">
               <TextBlock x:Name="MusicTileTitle" Text="{title}" FontSize="{TITLE_SIZE}"
                          TextWrapping="NoWrap" MaxLines="1"
                          LineHeight="{TITLE_LINE}" LineStackingStrategy="BlockLineHeight"
                          Foreground="{{ThemeResource SystemControlForegroundBaseHighBrush}}"/>
               <TextBlock x:Name="MusicTileArtist" Text="{artist}" FontSize="{ARTIST_SIZE}"
                          TextWrapping="NoWrap" MaxLines="1"
                          Foreground="{{ThemeResource SystemControlForegroundBaseMediumBrush}}"/>
             </StackPanel>
           </Border>"#,
        text_px = l.text,
        title = escape(&super::ticker::window(strip.display_title(), l.title_chars, 0)),
        artist = escape(&super::ticker::window(strip.display_artist(), l.artist_chars, 0)),
    );

    // Both namespaces and the transparent background are load-bearing (see `decorate::strip_markup`).
    // Never add a `ToolTipService.ToolTip`: it takes over hover and the shell's window preview
    // (where the transport buttons live) never opens.
    format!(
        r#"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        x:Name="MusicTileStrip" Height="32" Width="{strip_px}" Padding="{pad},0,{pad},0"
        Background="Transparent" HorizontalAlignment="Left">
  <StackPanel Orientation="Horizontal" HorizontalAlignment="Left">
    {cover}
    {text}
  </StackPanel>
</Border>"#,
        strip_px = l.strip,
        pad = l.pad,
    )
}

/// Where the shell's running indicator goes (it centres in the button by default, under the
/// text): the cover's centre, in epx from the strip's left edge.
pub fn icon_centre() -> f64 {
    let layout = layout();
    f64::from(layout.pad) + f64::from(layout.cover) / 2.0
}

/// The strip's full width, which the shell's progress bar spans (the whole plate, not the icon).
pub fn strip_width() -> f64 {
    f64::from(layout().strip)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A width too small for the roomy branch, which still has to lay out rather than underflow.
    const CRAMPED: u32 = 100;

    #[test]
    fn a_cramped_width_takes_the_smaller_fixed_parts() {
        let l = Layout::for_width(CRAMPED);
        assert_eq!((l.pad, l.cover, l.gap), (1, 26, 3));
        assert_eq!(l.text, 100 - (2 + 26 + 3 + 4));
    }

    /// The shipped width, and what it buys the parts.
    #[test]
    fn the_shipped_width_spends_what_is_left_on_the_column() {
        assert_eq!(STRIP_WIDTH, 150);
        let l = Layout::for_width(STRIP_WIDTH);
        assert_eq!((l.pad, l.cover, l.gap), (2, 28, 6));
        assert_eq!(l.text, 108, "the fixed parts took more than their 42 epx");
        assert_eq!((l.title_chars, l.artist_chars), (17, 20));
    }

    /// The idle label must not scroll. DirectWrite measures it at 99.8 / 73.5 epx.
    #[test]
    fn the_idle_label_still_fits_the_column() {
        let idle = super::super::state::Strip::default();
        let l = Layout::for_width(STRIP_WIDTH);
        for (label, chars) in [
            (idle.display_title(), l.title_chars),
            (idle.display_artist(), l.artist_chars),
        ] {
            assert!(
                !super::super::ticker::scrolls(label, chars),
                "{label:?} does not fit {chars} characters"
            );
        }
        assert!(f64::from(l.text) >= 99.81, "a {} epx column cannot hold the title", l.text);
    }

    /// The shipped width takes the generous branch.
    #[test]
    fn the_shipped_width_is_on_the_roomy_branch() {
        let shipped = Layout::for_width(STRIP_WIDTH);
        let generous = Layout::for_width(400);
        assert_eq!(
            (shipped.pad, shipped.cover, shipped.gap),
            (generous.pad, generous.cover, generous.gap)
        );
    }

    /// A full title window comes within a couple of epx of filling its column.
    #[test]
    fn a_full_title_window_very_nearly_fills_the_column() {
        let l = Layout::for_width(STRIP_WIDTH);
        let rendered = l.title_chars as f64 * TITLE_EPX_PER_CHAR as f64 / 100.0;
        let slack = f64::from(l.text) - rendered;
        assert!(slack < 4.0, "{slack} epx of dead space at the end of the title");
        // And it must not overflow so far that a whole character is wasted behind the clip.
        assert!(slack > -f64::from(TITLE_SIZE), "{slack}");
    }

    /// The two text lines fit the clip, or descenders are cut.
    #[test]
    fn the_two_lines_fit_inside_the_clip() {
        /// Natural line box of the artist line, measured: 14.6 epx at `ARTIST_SIZE`.
        const ARTIST_LINE: f64 = 14.6;
        assert!(
            f64::from(TITLE_LINE) + ARTIST_LINE <= f64::from(TEXT_HEIGHT),
            "{TITLE_LINE} + {ARTIST_LINE} > {TEXT_HEIGHT}"
        );
        // And the column cannot be taller than the strip, or the button clips it instead.
        const { assert!(TEXT_HEIGHT <= 32) };
    }

    /// The running indicator centres on the icon, not the strip.
    #[test]
    fn the_indicator_lands_under_the_icon() {
        let l = Layout::for_width(STRIP_WIDTH);
        assert_eq!(icon_centre(), f64::from(l.pad) + f64::from(l.cover) / 2.0);
        assert_eq!(icon_centre(), 16.0);
        assert!(icon_centre() < f64::from(l.pad + l.cover), "past the icon's right edge");
    }

    #[test]
    fn a_wider_slot_spends_it_on_the_text_column() {
        let narrow = Layout::for_width(CRAMPED);
        let wide = Layout::for_width(STRIP_WIDTH);
        assert!(wide.text > narrow.text, "{} vs {}", wide.text, narrow.text);
        // The fixed parts grow too, but only once there is room for them.
        assert!(wide.cover > narrow.cover && wide.gap > narrow.gap);
    }

    /// The parts never sum past the strip, at any width.
    #[test]
    fn the_parts_always_fit_the_budget() {
        for width in CRAMPED..600 {
            let l = Layout::for_width(width);
            let used = 2 * l.pad + l.cover + l.gap + l.text;
            assert!(used <= width, "{width}: parts use {used}");
        }
    }

    /// No transport glyphs and, critically, no tooltip (it silently suppresses the window preview).
    #[test]
    fn the_strip_is_just_a_label_now() {
        let markup = now_playing_markup(&super::super::state::Strip::default());
        for name in ["MusicTilePrevious", "MusicTilePlayPause", "MusicTileNext"] {
            assert!(!markup.contains(name), "{name} is still on the strip");
        }
        assert!(
            !markup.contains("ToolTipService"),
            "a tooltip here suppresses the shell's hover preview entirely"
        );
    }

    /// The strip still has to survive the awkward inputs: no cover, and text carrying markup.
    #[test]
    fn the_strip_escapes_and_falls_back() {
        let bare = now_playing_markup(&super::super::state::Strip::default());
        assert!(bare.contains("Nothing playing"), "{bare}");
        assert!(!bare.contains("BitmapImage"), "no cover means no image source");

        let awkward = now_playing_markup(&super::super::state::Strip {
            title: "Sturm & Drang".into(),
            artist: "<script>".into(),
            ..Default::default()
        });
        assert!(awkward.contains("Sturm &amp; Drang"), "{awkward}");
        assert!(awkward.contains("&lt;script&gt;"), "{awkward}");
    }
}
