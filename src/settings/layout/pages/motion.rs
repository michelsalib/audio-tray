//! The Motion page: how the bars move, each part on its own, picked by what it looks like.

use crate::config::{MotionStyle, WaveColour};
use crate::lang::tr;

use super::super::choice::WAVE_COLOURS;
use super::super::column::{choice_tiles, Column, Gate, Tile};
use super::super::{Choice, Model, Page, Picture};

/// A picture tile's height on Motion.
const TILE_H: f32 = 150.0;

/// How the bars move, each part on its own, picked by what it looks like.
pub(in crate::settings::layout) fn motion(col: &mut Column, model: &Model) {
    let m = model.config.motion;
    let pill_off = Gate::needs(!model.config.pill.enabled, tr("Needs the edge pill", "Nécessite la pastille"), Some(Page::Pill));
    let row = |picture: fn(MotionStyle) -> Picture, current: MotionStyle, choice: Choice, words: [&'static str; 3]| -> Vec<Tile> {
        let names = [tr("None", "Aucune"), "Google", "Samsung"];
        let options: Vec<_> = MotionStyle::ALL.into_iter().zip(names).zip(words).map(|((style, name), line)| (style, name, line)).collect();
        choice_tiles(&options, &MotionStyle::ALL, current, picture, choice)
    };
    col.gap(4.0);
    let ring = row(Picture::Ring, m.ring, Choice::RingMotion, [tr("Fills, nothing more", "Se remplit, sans plus"), tr("Ripples; flat on pause", "Ondule ; plat en pause"), tr("Layered waves", "Vagues superposées")]);
    col.tiles(Some((tr("Progress ring", "Anneau de progression"), tr("Around each cover on the pill", "Autour de chaque pochette sur la pastille"))), &ring, pill_off, TILE_H);
    let bar = row(Picture::Bar, m.bar, Choice::BarMotion, [tr("A plain bar", "Une barre simple"), tr("A rippling line", "Une ligne ondulante"), tr("Waves over the line", "Des vagues sur la ligne")]);
    col.tiles(Some((tr("Track bar", "Barre de lecture"), tr("Under each song on the card", "Sous chaque morceau sur la carte"))), &bar, pill_off, TILE_H);
    let volume = row(Picture::Volume, m.volume, Choice::VolumeMotion, [tr("Jumps to the level", "Saute au niveau"), tr("Springs to the level", "Rebondit jusqu’au niveau"), tr("A stretchy capsule", "Une capsule élastique")]);
    col.tiles(Some((tr("Volume sliders", "Curseurs de volume"), tr("In the panel and readout", "Dans le panneau et l’indicateur"))), &volume, Gate::Open, TILE_H);
    let colours = [
        (WaveColour::Accent, "Accent", tr("The Windows accent", "La couleur d’accent de Windows")),
        (WaveColour::Cover, tr("Cover", "Pochette"), tr("Each cover's own colour", "La couleur de chaque pochette")),
    ];
    let colours = choice_tiles(&colours, &WAVE_COLOURS, m.colour, Picture::Colour, Choice::WaveColour);
    col.tiles(Some((tr("Wave colour", "Couleur de la vague"), tr("For the ring and the bar", "Pour l’anneau et la barre"))), &colours, pill_off, TILE_H);
}
