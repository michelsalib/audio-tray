//! The strip's geometry and its XAML, both derived from one number: how wide the strip is.
//!
//! Ported from audio-tray, where every constant here was measured on a real taskbar rather than
//! chosen. The two that cost the most to get right:
//!
//! * **Characters per epx** (`TITLE_EPX_PER_CHAR`) is read off *rendered* text. Eyeballed values were
//!   14 % too wide, which left 16 epx of empty strip between the title and the transport buttons.
//! * **The text column's height** is 32 with the title's line box forced to 17. The natural boxes are
//!   18.6 + 14.6 = 33.2, which in a 30-epx column cut the descenders off the artist name.
//!
//! Nothing here touches XAML or the tree: it is a pure function of the width, which is why it can be
//! unit-tested, and it is.

/// The width the strip lays itself out in.
///
/// **162, down from 240, because the transport controls left.** They are on the shell's own
/// thumbnail toolbar under the hover preview now (`music::thumbbar`, audio-tray side), which frees
/// the 78 epx three buttons occupied. That width goes back to the taskbar rather than into the text
/// column: the column keeps the 120 epx it was measured and calibrated at, and the app icons stop
/// being squeezed toward the shell's compressed layout.
///
/// ```text
/// 2·pad 4  +  cover 28  +  gap 6  +  text 120  +  slack 4  =  162
/// ```
pub const STRIP_WIDTH: u32 = 162;

// How much wider than the strip the button has to be asked for is a property of *which* button —
// 80 epx for the Widgets entry point, 4 for a task button — so it lives on `super::tile::Host` rather
// than here. See `tile::Host::slot_overhead`.

/// Font sizes, in effective pixels.
///
/// Raised from 10/8, which was measured as "painful to read" at arm's length. The cost is
/// paid in visible characters, not in layout: the column is a fixed width either way, and the
/// ticker scrolls whatever does not fit.
pub const TITLE_SIZE: u32 = 14;
pub const ARTIST_SIZE: u32 = 11;

/// The text column's height, and the title's line box — both measured, because the two of them
/// together are what clipped the artist's descenders.
///
/// Natural line boxes at these font sizes are 18.6 and 14.6 epx: 33.2 stacked, in a column that was
/// 30 tall with a clip to match, so the bottom 3.2 epx went — the tails of `p` and `y` in
/// `Periphery`. The strip is 32 epx tall and the button gives no more, so the fix is to spend the
/// two spare epx and take the rest out of the *leading* rather than the glyphs:
///
/// ```text
/// column 32   =   title line 17 (BlockLineHeight, was 18.6)   +   artist 14.6 natural
/// ```
///
/// `LineStackingStrategy="BlockLineHeight"` is what makes `LineHeight` binding rather than a
/// minimum; without it the line box stays at its natural 18.6 and nothing moves.
const TEXT_HEIGHT: u32 = 32;
const TITLE_LINE: u32 = 17;

/// Effective pixels per character, at [`TITLE_SIZE`] and [`ARTIST_SIZE`], × 100.
///
/// **Measured off the rendered text, not guessed.** These were 743 and 578, eyeballed from the
/// 144 epx layout, and being 14 % too wide showed up as a visible hole: a 16-character title in a
/// 120 epx column rendered `MusicTileTitle` at **103.9 epx**, leaving 16 epx of empty strip between
/// the text and the transport buttons — reported as "we lose a lot of space". `MusicTileArtist` came
/// to 47.5 epx for the 9 characters of `Periphery`.
///
/// ```text
/// title   103.9 / 16 = 6.49 epx per character   (was 7.43)
/// artist   47.5 /  9 = 5.28 epx per character   (was 5.78)
/// ```
///
/// A character count rather than a measured width still, because the font is proportional and there
/// is no cheap way to measure text from inside the TAP — but the *average* has to be right, or the
/// column is either part empty or scrolling text that would have fit. An average also means a title
/// of unusually wide characters overflows; that is what the `Clip` is for, and overflowing by a
/// character is the better error, since the alternative is dead space on every normal title.
const TITLE_EPX_PER_CHAR: u32 = 649;
const ARTIST_EPX_PER_CHAR: u32 = 528;

/// The strip's internal geometry, derived from the width it has to fill.
///
/// Everything here used to be a constant tuned by hand for 144 epx. It is computed now because
/// M11 showed the slot is not fixed: the same code has to lay out 144 and 240 without a second
/// set of hand-tuned numbers, and every part has to move together or the space goes to whichever
/// element happened to be hardcoded largest.
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
    /// Fit the strip's parts into `strip` epx.
    ///
    /// The fixed parts get their size from how much room there is — a 144 epx strip cannot
    /// afford a 28 epx cover and 26 epx buttons, and a 240 epx one looks starved with 26 and 19.
    /// Whatever is left goes to the text column, because that is the only part with a graceful
    /// response to being short: the ticker scrolls it.
    pub fn for_width(strip: u32) -> Self {
        // **Recalibrated when the transport buttons left the strip.** The old threshold was 200 with
        // three 26-epx buttons in the budget; the same amount of room for the parts that remain is
        // 200 − 78 = 122. Keeping 200 here would have made the new 162-epx strip take the cramped
        // branch and shrink a cover that now has more space, not less.
        let roomy = strip >= 122;
        let pad = if roomy { 2 } else { 1 };
        let cover = if roomy { 28 } else { 26 };
        let gap = if roomy { 6 } else { 3 };

        // **The slack is load-bearing, not rounding.** Segoe Fluent glyph ink overshoots its
        // layout box, so without it the trailing bar of the `next` glyph clips — measured twice
        // at 144. It is kept now the glyphs have gone because the same overshoot applies to the
        // text column's last character against the plate's rounded corner.
        const SLACK: u32 = 4;
        let fixed = 2 * pad + cover + gap + SLACK;
        let text = strip.saturating_sub(fixed);

        Self {
            strip,
            pad,
            cover,
            gap,
            text,
            // Rounded, not truncated: the 144 epx column is 7.0 title characters exactly, and
            // integer division would call it 6 and quietly scroll text that used to fit.
            title_chars: ((text * 100 + TITLE_EPX_PER_CHAR / 2) / TITLE_EPX_PER_CHAR) as usize,
            artist_chars: ((text * 100 + ARTIST_EPX_PER_CHAR / 2) / ARTIST_EPX_PER_CHAR) as usize,
        }
    }
}

/// The width the strip lays its content out in — [`STRIP_WIDTH`] unless `strip=<epx>` was passed.
///
/// Deliberately separate from the `widen=` that opens the slot: one asks the shell for room, the
/// other decides what to draw in it, and the measured gap between the two (ask 320, paint 249)
/// means they cannot be the same number. Content wider than what paints would clip the `next`
/// glyph — the exact defect the slack exists to prevent.
static CONTENT_WIDTH: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(STRIP_WIDTH);

pub fn set_content_width(width: u32) {
    CONTENT_WIDTH.store(width, std::sync::atomic::Ordering::SeqCst);
}

/// The live layout. Cheap enough to recompute per use — it is a handful of integer operations,
/// and a cached copy would be one more thing to invalidate.
pub fn layout() -> Layout {
    Layout::for_width(CONTENT_WIDTH.load(std::sync::atomic::Ordering::SeqCst))
}

// Historical, kept because the reasoning still applies to any future width experiment: the M11
// `widen` probe had the root plate ask for more epx than [`STRIP_WIDTH`] and paint itself, so a
// screenshot showed exactly where the slot cut it off — with the *contents* keeping their own
// budget either way. Putting the experiment in the plate rather than in the layout is what made a
// refused slot leave a working strip instead of a clipped one, which is what had made the previous
// attempt ambiguous.

/// The strip drawn into the app's taskbar button: cover, title, artist.
///
/// The transport controls are on the shell's thumbnail toolbar under the hover preview now — this
/// surface is 162 epx of taskbar and spends all of it on saying what is playing.
pub fn now_playing_markup(strip: &super::state::Strip) -> String {
    use super::state::escape;

    let l = layout();

    // A cover if there is one, otherwise a note glyph on a muted plate — a hole where the
    // art should be looks like a bug, and plenty of sessions publish no artwork.
    // Both the art and its placeholder are always present, stacked, with `Visibility`
    // choosing between them. That is what lets a track change be a handful of property
    // writes instead of a rebuild — and rebuilding is what made the weather flash through
    // and what dropped clicks mid-press, because it replaces every element.
    let cover = format!(
        r#"<Grid Width="{cover_px}" Height="{cover_px}" Margin="0,0,{gap},0">
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
        cover_px = l.cover,
        gap = l.gap,
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
    );

    // The text column is a **fixed** width, not a `MaxWidth`, and that is the fix for a real
    // defect: with `MaxWidth` the column grew with the content, so a long artist name pushed
    // the transport buttons sideways and off the end — the layout moved under the pointer
    // depending on what was playing. Fixed width means the buttons never move.
    //
    // The `Clip` is what makes overflow disappear cleanly instead of spilling over the
    // buttons: XAML panels do not clip their children, so without it a long name simply
    // draws on top of everything to its right. Text longer than the column is scrolled by
    // [`super::ticker`] rather than ellipsised, so it stays readable.
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

    // Both namespaces, and `Background="Transparent"` on the plate — see `strip_markup` for why
    // each is load-bearing.
    //
    // **No `ToolTipService.ToolTip` here, and that is a requirement rather than a taste.** Measured:
    // a tooltip declared on this element makes XAML's tooltip service own hover for the whole
    // subtree, and the shell's `Taskbar.FlyoutFrame` preview then never opens at all — the tile was
    // the one taskbar button with no window preview, and nothing said so. Removing it brings the
    // preview straight back, which is also where the transport controls now are.
    //
    // **No transport glyphs either.** They are on the shell's thumbnail toolbar under that preview,
    // where they cost no taskbar width. What is left is the thing a taskbar button should be: an
    // icon and a label saying what is playing.
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

/// Where the shell's running indicator has to sit to be under the strip's app icon: the centre of
/// the cover square, in epx from the strip's left edge.
///
/// The shell centres that indicator in the *button*, which is fine at 44 epx and wrong at 244 —
/// centred there, it lands under the middle of the title text and reads as a stray dot. This is the
/// one number needed to move it back under the icon, and it is the layout's, not a guess.
pub fn icon_centre() -> f64 {
    let layout = layout();
    f64::from(layout.pad) + f64::from(layout.cover) / 2.0
}

/// The icon's width.
pub fn icon_width() -> f64 {
    f64::from(layout().cover)
}

/// The strip's full width — what the shell's progress bar is sized to.
///
/// **The whole plate, not the icon.** Sizing the bar to the 28-epx cover was where it started, on the
/// reasoning that it should sit under the app icon the way MPC-HC's does. On a strip that also
/// carries the title and artist it reads as a stray underline instead: the bar is about the *track*,
/// and the track is the whole strip. Spanning the plate makes it a progress bar for the thing the
/// plate is showing, which is also the only reading at which its length means anything at a glance.
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

    /// **The point of the move.** The transport buttons left for the hover preview, and the width
    /// they occupied went back to the taskbar rather than into the text — the column keeps exactly
    /// the 120 epx it was calibrated at, and the strip is 78 epx narrower than it was.
    #[test]
    fn the_default_width_keeps_the_calibrated_text_column() {
        assert_eq!(STRIP_WIDTH, 162);
        let l = Layout::for_width(STRIP_WIDTH);
        assert_eq!((l.pad, l.cover, l.gap), (2, 28, 6));
        assert_eq!(l.text, 120, "the text column must survive the narrowing untouched");
        assert_eq!((l.title_chars, l.artist_chars), (18, 23));
        // For the record: 162 + three 26-epx buttons is the 240 this strip used to be.
    }

    /// The recalibrated `roomy` threshold has to put the shipped width on the generous branch.
    /// At the old 200 it would have taken the cramped one and shrunk a cover that gained room.
    #[test]
    fn the_shipped_width_is_on_the_roomy_branch() {
        let shipped = Layout::for_width(STRIP_WIDTH);
        let generous = Layout::for_width(400);
        assert_eq!(
            (shipped.pad, shipped.cover, shipped.gap),
            (generous.pad, generous.cover, generous.gap)
        );
    }

    /// **The dead space this recalibration exists to remove.** A title window has to come within a
    /// couple of epx of filling its column: 16 characters rendered 103.9 epx of a 120 epx column,
    /// and the 16 left over read as the strip losing space before the transport buttons.
    #[test]
    fn a_full_title_window_very_nearly_fills_the_column() {
        let l = Layout::for_width(STRIP_WIDTH);
        let rendered = l.title_chars as f64 * TITLE_EPX_PER_CHAR as f64 / 100.0;
        let slack = f64::from(l.text) - rendered;
        assert!(slack < 4.0, "{slack} epx of dead space at the end of the title");
        // And it must not overflow so far that a whole character is wasted behind the clip.
        assert!(slack > -f64::from(TITLE_SIZE), "{slack}");
    }

    /// The two text lines have to fit the column they are clipped to, or descenders are cut — which
    /// is exactly what a 30 epx column did to the tail of `Periphery`.
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

    /// The running indicator is placed against this, so it has to be the icon's centre and not the
    /// strip's: a 240-epx strip centred its indicator at 120, under the title text, which is the
    /// defect that made a real "the app is open" cue read as a stray dot.
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

    /// The parts must never sum past the strip, at any width: overflowing is not a cosmetic
    /// problem but the last of the text falling outside the plate's rounded corner.
    #[test]
    fn the_parts_always_fit_the_budget() {
        for width in CRAMPED..600 {
            let l = Layout::for_width(width);
            let used = 2 * l.pad + l.cover + l.gap + l.text;
            assert!(used <= width, "{width}: parts use {used}");
        }
    }

    /// The strip carries no transport glyphs — those are the shell's thumbnail toolbar now — and,
    /// critically, **no tooltip**.
    ///
    /// The tooltip is the one that would fail silently: declaring it makes XAML's tooltip service own
    /// hover for this subtree, and the shell's window preview then never opens. That cost the tile
    /// the very surface the controls were moved to, and nothing in any log said so.
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
