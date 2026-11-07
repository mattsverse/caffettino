use crate::{CoffeeBreak, State};
use rune::tok::{TextStyle, motion};
use rune::{Color, Cursor, Cx, El, LineCap, Path, Shape, canvas, col, el, row, text};

pub const SIZE: (f32, f32) = (380.0, 600.0);
// sRGB #faf6ed, decoded to Rune's linear color space.
const PAPER: Color = Color {
    r: 0.955_973,
    g: 0.921_582,
    b: 0.846_873,
    a: 1.0,
};
// sRGB #41332d, decoded to Rune's linear color space.
const INK: Color = Color {
    r: 0.052_861,
    g: 0.033_105,
    b: 0.026_241,
    a: 1.0,
};
// sRGB #7b6c60, decoded to Rune's linear color space.
const MUTED: Color = Color {
    r: 0.198_069,
    g: 0.149_960,
    b: 0.116_971,
    a: 1.0,
};
// sRGB #794a36, decoded to Rune's linear color space.
const COFFEE: Color = Color {
    r: 0.191_202,
    g: 0.068_478,
    b: 0.036_889,
    a: 1.0,
};
// sRGB #416b50, decoded to Rune's linear color space.
const GREEN: Color = Color {
    r: 0.052_861,
    g: 0.147_027,
    b: 0.080_220,
    a: 1.0,
};
// sRGB #eee5d4, decoded to Rune's linear color space.
const CREAM: Color = Color {
    r: 0.854_993,
    g: 0.783_538,
    b: 0.658_375,
    a: 1.0,
};

fn rgb(hex: u32) -> Color {
    let [_, red, green, blue] = hex.to_be_bytes();
    Color {
        r: rune::color::srgb_decode(f32::from(red) / 255.0),
        g: rune::color::srgb_decode(f32::from(green) / 255.0),
        b: rune::color::srgb_decode(f32::from(blue) / 255.0),
        a: 1.0,
    }
}

fn label(value: impl Into<String>, size: f32, weight: f32, color: Color) -> El {
    text(value).font(TextStyle::new(size, weight)).color(color)
}

fn cup(awake: bool) -> Shape {
    let body = Path::new()
        .move_to([55.0, 82.0])
        .line_to([151.0, 82.0])
        .line_to([144.0, 143.0])
        .quad_to([103.0, 170.0], [62.0, 143.0])
        .close();
    let handle =
        Path::new()
            .move_to([150.0, 91.0])
            .cubic_to([192.0, 81.0], [194.0, 139.0], [147.0, 135.0]);
    let rim = Path::new()
        .move_to([55.0, 82.0])
        .quad_to([103.0, 66.0], [151.0, 82.0])
        .quad_to([103.0, 99.0], [55.0, 82.0])
        .close();
    let smile = Path::new()
        .move_to([93.0, 130.0])
        .quad_to([103.0, 140.0], [113.0, 130.0]);
    let mut shape = Shape::new()
        .stroke_path(&handle, 9.0, LineCap::Round, COFFEE)
        .fill_path(
            &body,
            if awake {
                rgb(0xd9_9b_74)
            } else {
                rgb(0xd8_c9_b3)
            },
        )
        .stroke_path(&body, 3.0, LineCap::Round, COFFEE)
        .fill_path(&rim, COFFEE)
        .stroke(
            vec![[45.0, 169.0], [66.0, 175.0], [142.0, 175.0], [163.0, 169.0]],
            3.0,
            COFFEE,
        )
        .stroke_path(&smile, 2.5, LineCap::Round, COFFEE);
    for x in [85.0, 121.0] {
        shape = if awake {
            shape.stroke(vec![[x, 113.0], [x, 119.0]], 4.0, INK)
        } else {
            shape.stroke(
                vec![[x - 5.0, 116.0], [x, 119.0], [x + 5.0, 116.0]],
                2.5,
                COFFEE,
            )
        };
    }
    if awake {
        for x in [80.0, 105.0, 130.0] {
            let steam = Path::new().move_to([x, 61.0]).cubic_to(
                [x - 17.0, 45.0],
                [x + 15.0, 38.0],
                [x, 22.0],
            );
            shape = shape.stroke_path(&steam, 3.0, LineCap::Round, rgb(0xb8_8a_65));
        }
    } else {
        let z = |x, y, scale| {
            vec![
                [x, y],
                [x + scale, y],
                [x, y + scale],
                [x + scale, y + scale],
            ]
        };
        shape =
            shape
                .stroke(z(141.0, 50.0, 9.0), 2.0, MUTED)
                .stroke(z(159.0, 31.0, 13.0), 2.0, MUTED);
    }
    shape.view_box(220.0, 200.0)
}

fn duration_selector(state: &State) -> El {
    let active = state.active();
    let mut durations = row()
        .role("radiogroup")
        .label("Coffee break duration")
        .focus_group()
        .w_full()
        .h(42.0)
        .padding(4.0)
        .gap(4.0)
        .radius(13.0)
        .fill(CREAM);
    for (index, coffee_break) in CoffeeBreak::ALL.into_iter().enumerate() {
        let selected = coffee_break == state.coffee_break;
        durations = durations.child(
            el().key(format!("duration-{index}"))
                .role("radio")
                .checked(selected)
                .focus_selected(selected)
                .label(coffee_break.label())
                .disabled(active)
                .focusable(!active)
                .grow(1.0)
                .h_full()
                .radius(9.0)
                .center()
                .fill(if selected { PAPER } else { Color::TRANSPARENT })
                .cursor(if active {
                    Cursor::Default
                } else {
                    Cursor::Pointer
                })
                .motion(motion::SNAPPY)
                .on_press(move |s: &mut State| s.select_break(coffee_break))
                .child(label(
                    coffee_break.label(),
                    12.0,
                    if selected { 600.0 } else { 450.0 },
                    INK,
                )),
        );
    }
    durations
}

fn brew_button(active: bool) -> El {
    el().key("brew")
        .role("button")
        .label(if active {
            "Stop caffeinate"
        } else {
            "Start caffeinate"
        })
        .focusable(true)
        .w_full()
        .h(48.0)
        .radius(14.0)
        .fill(if active { GREEN } else { COFFEE })
        .hover(rgb(0x8b_61_4d))
        .cursor(Cursor::Pointer)
        .press_scale(0.98)
        .on_press_tx(State::toggle)
        .child(
            label(
                if active {
                    "Let it snooze"
                } else {
                    "Brew a little focus"
                },
                14.0,
                600.0,
                PAPER,
            )
            .self_center(),
        )
        .center()
}

fn footer(active: bool) -> El {
    col()
        .w_full()
        .center()
        .gap(5.0)
        .child(label(
            if active {
                "Display & Mac stay awake. Closing this keeps brewing."
            } else {
                "Keeps your display & Mac awake while you're here."
            },
            10.0,
            450.0,
            MUTED,
        ))
        .child(label(
            "Menu bar cup: click to toggle · right-click for more",
            10.0,
            450.0,
            MUTED,
        ))
}

pub fn view(state: &State, _: &mut Cx<State>) -> El {
    let active = state.active();
    col()
        .key("root")
        .w_full()
        .h_full()
        .fill(PAPER)
        .padding(24.0)
        .gap(16.0)
        .child(
            row()
                .w_full()
                .h(24.0)
                .justify_end()
                .window_drag()
                .child(label("caffettino", 13.0, 650.0, COFFEE)),
        )
        .child(
            col()
                .w_full()
                .h(215.0)
                .radius(24.0)
                .fill(rgb(0xf2_ea_db))
                .center()
                .child(
                    canvas(cup(active))
                        .key("cup")
                        .size(220.0, 200.0)
                        .rotate(if active { -0.035 } else { 0.035 })
                        .motion(motion::CALM),
                ),
        )
        .child(
            col()
                .w_full()
                .center()
                .gap(8.0)
                .child(
                    row()
                        .gap(6.0)
                        .center()
                        .child(el().size(6.0, 6.0).radius(3.0).fill(if active {
                            GREEN
                        } else {
                            MUTED
                        }))
                        .child(label(
                            if active {
                                "FRESHLY CAFFEINATED"
                            } else {
                                "OFF DUTY, ON PURPOSE"
                            },
                            10.0,
                            650.0,
                            if active { GREEN } else { MUTED },
                        )),
                )
                .child(label(
                    if active {
                        "Wide awake, little Mac."
                    } else {
                        "A tiny cup. A big help."
                    },
                    23.0,
                    650.0,
                    INK,
                ))
                .child(label(state.detail(), 11.5, 450.0, MUTED).key("detail")),
        )
        .child(
            col()
                .w_full()
                .gap(8.0)
                .child(label(
                    if active {
                        "THIS COFFEE BREAK"
                    } else {
                        "KEEP ME COMPANY FOR"
                    },
                    10.0,
                    600.0,
                    MUTED,
                ))
                .child(duration_selector(state)),
        )
        .child(brew_button(active))
        .child(footer(active))
}
