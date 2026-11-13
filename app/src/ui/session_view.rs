use super::*;

impl Telinha {
    pub(super) fn top(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let compact = window.viewport_size().width < px(1040.);
        let mut context = div().flex().flex_col().min_w_0().gap(px(2.));
        if let Some(call) = &self.auto_join.active {
            context = context
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(format!("# {}", call.channel_name)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(call.guild_name.clone()),
                );
        } else {
            context = context
                .child(
                    div()
                        .font_family(mono())
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(
                            self.room
                                .as_ref()
                                .map(RoomCode::grouped)
                                .unwrap_or_default(),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Sessão por link"),
                );
        }
        div()
            .h(px(68.))
            .flex_shrink_0()
            .px_6()
            .flex()
            .items_center()
            .gap_4()
            .child(
                svg()
                    .path("logo.svg")
                    .w(px(72.))
                    .h(px(22.))
                    .text_color(rgb(FG)),
            )
            .child(div().w(px(1.)).h(px(26.)).bg(rgb(LINE)))
            .child(div().flex_1().min_w_0().child(context))
            .child(self.people_menu(cx))
            .child(
                button_with_icon(
                    "invite",
                    if compact {
                        "Convidar"
                    } else {
                        "Convidar por link"
                    },
                    "icons/link.svg",
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.copy(true, cx))),
            )
            .child(self.account_bar(cx))
            .into_any_element()
    }

    pub(super) fn stage(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let camera_count = self.camera_count();
        let camera_gallery =
            camera_count > 0 && self.streams.is_empty() && self.publishing.is_none();
        let mut well = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .bg(rgb(0x17181A))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .when(!self.fullscreen, |well| well.rounded(px(10.)));
        if let Some(image) = &self.image {
            well = well.bg(rgb(0)).child(
                img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            );
            if let Some(stream) = self
                .streams
                .iter()
                .find(|s| self.backend.media.selected_track.lock().as_ref() == Some(&s.id))
            {
                well = well.child(
                    div()
                        .absolute()
                        .bottom_4()
                        .left_4()
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .bg(gpui::rgba(0x17181AD9))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(self.avatar(stream.avatar_url.as_deref(), &stream.title, 24.))
                        .child(div().max_w(px(260.)).truncate().child(stream.title.clone()))
                        .child(div().ml_1().size(px(6.)).rounded_full().bg(rgb(LIVE))),
                );
            }
        } else if camera_gallery {
            let columns = if camera_count == 1 { 1 } else { 2 };
            let rows = camera_count.div_ceil(columns);
            let available = f32::from(window.viewport_size().height) - 190.;
            let height = (available / rows as f32 - 12.).clamp(140., 480.);
            well = well.child(
                div()
                    .id("camera-gallery")
                    .size_full()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_4()
                    .grid()
                    .grid_cols(columns as u16)
                    .gap_3()
                    .children(self.camera_tiles(height)),
            );
        } else if !self.streams.is_empty() {
            well = well.child(empty_state(
                "icons/monitor.svg",
                "Recebendo a transmissão",
                "O vídeo aparece aqui em instantes.",
            ));
        } else if let Some(name) = &self.publishing {
            well =
                well.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_5()
                        .p_8()
                        .max_w(px(600.))
                        .child(
                            div()
                                .size(px(64.))
                                .rounded(px(16.))
                                .bg(gpui::rgba(0xF0443A14))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("icons/share.svg", 30., LIVE)),
                        )
                        .child(
                            div()
                                .text_size(px(24.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Sua tela está no ar"),
                        )
                        .child(
                            div()
                                .text_color(rgb(MUTED))
                                .text_center()
                                .max_w(px(480.))
                                .child(name.clone()),
                        )
                        .child(div().text_sm().text_color(rgb(MUTED)).text_center().child(
                            "Você também pode assistir às transmissões dos seus amigos aqui.",
                        )),
                );
        } else if self.auto_join.active.is_some() || self.participants.len() > 1 {
            well = well.child(empty_state(
                "icons/monitor.svg",
                "Ninguém está transmitindo",
                "Compartilhe sua tela ou espere alguém começar.",
            ));
        } else {
            let mut empty = div()
                .flex()
                .flex_col()
                .items_center()
                .gap_5()
                .p_8()
                .max_w(px(620.))
                .child(
                    div()
                        .size(px(56.))
                        .rounded(px(14.))
                        .bg(rgb(BG))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("icons/link.svg", 26., MUTED)),
                )
                .child(
                    div()
                        .text_size(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Sua sessão está pronta"),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Mande o link para quem vai assistir com você."),
                );
            if let Some(room) = &self.room {
                empty = empty
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .px_4()
                                    .h(px(44.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(8.))
                                    .bg(rgb(BG))
                                    .font_family(mono())
                                    .text_sm()
                                    .child(format!("telinha.tech/j/{}", room.as_str())),
                            )
                            .child(
                                button_with_icon(
                                    "copy-link",
                                    "Copiar link",
                                    "icons/copy.svg",
                                    true,
                                )
                                .h(px(44.))
                                .on_click(cx.listener(|this, _, _, cx| this.copy(true, cx))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child("Ou use o código"),
                            )
                            .child(div().font_family(mono()).text_sm().child(room.grouped()))
                            .child(
                                quiet_button("copy-code", "Copiar")
                                    .on_click(cx.listener(|this, _, _, cx| this.copy(false, cx))),
                            ),
                    );
            }
            well = well.child(empty);
        }
        let mut stage = div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .gap_3()
            .when(!self.fullscreen, |stage| stage.mx_4())
            .child(well);
        if (!self.fullscreen && self.streams.len() > 1) || (camera_count > 0 && !camera_gallery) {
            let mut list = div()
                .id("stream-list")
                .w(px(if camera_count > 0 { 220. } else { 184. }))
                .flex_shrink_0()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_2()
                .when(!self.streams.is_empty() && !self.fullscreen, |list| {
                    list.child(
                        div()
                            .px_2()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("Transmissões · {}", self.streams.len())),
                    )
                });
            for stream in self.streams.iter().filter(|_| !self.fullscreen) {
                let id = stream.id.clone();
                let active = self.backend.media.selected_track.lock().as_ref() == Some(&id);
                list = list.child(
                    div()
                        .id(SharedString::from(id.clone()))
                        .p_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(rgb(if active { FG } else { LINE }))
                        .bg(rgb(if active { PANEL } else { BG }))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(PANEL)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.avatar(
                                    stream.avatar_url.as_deref(),
                                    &stream.title,
                                    28.,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .truncate()
                                        .child(stream.title.clone()),
                                ),
                        )
                        .child(
                            div()
                                .mt_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(div().size(px(5.)).rounded_full().bg(rgb(LIVE)))
                                .child(if active {
                                    "Assistindo agora"
                                } else {
                                    "Clique para assistir"
                                }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_stream(Some(id.clone()), cx)
                        })),
                );
            }
            if camera_count > 0 {
                list = list
                    .child(
                        div()
                            .px_2()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("Webcams · {camera_count}")),
                    )
                    .children(self.camera_tiles(150.));
            }
            stage = stage.child(list);
        }
        if self.fullscreen && self.controls_visible {
            stage = stage.child(
                div()
                    .absolute()
                    .bottom_4()
                    .left_4()
                    .right_4()
                    .child(self.controls(window, cx)),
            );
        }
        stage.into_any_element()
    }

    fn volume_slider(&self, cx: &mut Context<Self>) -> AnyElement {
        let bounds = self.volume_bounds.clone();
        let width = 96.;
        let fill = width * f32::from(self.volume) / 100.;
        div()
            .id("volume-slider")
            .relative()
            .w(px(width))
            .h(px(32.))
            .cursor_pointer()
            .child(
                canvas(move |area, _, _| bounds.set(area), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(14.))
                    .h(px(4.))
                    .rounded_full()
                    .bg(rgb(LINE)),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(14.))
                    .w(px(fill))
                    .h(px(4.))
                    .rounded_full()
                    .bg(rgb(FG)),
            )
            .child(
                div()
                    .absolute()
                    .left(px((fill - 5.).clamp(-5., width - 5.)))
                    .top(px(10.))
                    .size(px(12.))
                    .rounded_full()
                    .bg(rgb(FG)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.dragging_volume = true;
                    this.volume_at(event.position.x, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging_volume = false),
            )
            .into_any_element()
    }

    pub(super) fn controls(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let compact = window.viewport_size().width < px(1180.);
        let mut row = div()
            .h(px(72.))
            .flex_shrink_0()
            .px_5()
            .flex()
            .items_center()
            .gap_3()
            .when(self.fullscreen, |row| {
                row.bg(gpui::rgba(0x2A2C30F2)).rounded(px(12.))
            });
        if !self.streams.is_empty() {
            row = row
                .child(
                    button_with_icon(
                        "mute",
                        "",
                        if self.volume == 0 {
                            "icons/mute.svg"
                        } else {
                            "icons/volume.svg"
                        },
                        false,
                    )
                    .w(px(36.))
                    .px_0()
                    .border_color(gpui::rgba(0))
                    .bg(gpui::rgba(0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = if this.volume == 0 {
                            this.previous_volume.max(1)
                        } else {
                            0
                        };
                        this.set_volume(next, cx);
                    })),
                )
                .child(self.volume_slider(cx))
                .when(!compact, |row| {
                    row.child(
                        div()
                            .w(px(34.))
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("{}%", self.volume)),
                    )
                })
                .child(
                    button_with_icon(
                        "fullscreen",
                        if compact {
                            ""
                        } else if self.fullscreen {
                            "Reduzir"
                        } else {
                            "Tela cheia"
                        },
                        if self.fullscreen {
                            "icons/restore.svg"
                        } else {
                            "icons/fullscreen.svg"
                        },
                        false,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.toggle_fullscreen();
                        this.fullscreen = !this.fullscreen;
                        this.controls_visible = true;
                        this.mouse_at = Instant::now();
                        cx.notify();
                    })),
                );
        }
        if self.publishing.is_some() {
            row = row.child(live_badge()).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_sm().child(format!("{} fps", self.fps)))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(if compact {
                        "capturados"
                    } else {
                        self.quality.label()
                    })),
            );
        } else if self.streams.is_empty() && !compact {
            row = row.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Tudo pronto para compartilhar"),
            );
        }
        row = row.child(div().flex_1());
        let camera_on = self.camera_publishing.is_some() || self.camera_starting;
        row = row.child(
            button_with_colored_icon(
                "webcam",
                if self.camera_starting {
                    "Cancelar webcam"
                } else if camera_on {
                    "Desligar webcam"
                } else {
                    "Ligar webcam"
                },
                if camera_on {
                    "icons/camera-off.svg"
                } else {
                    "icons/camera.svg"
                },
                false,
                if camera_on { LIVE } else { FG },
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.camera_publishing.is_some() || this.camera_starting {
                    this.stop_camera(cx);
                } else {
                    this.choose_camera(cx);
                }
            })),
        );
        if self.publishing.is_some() {
            row = row
                .child(
                    button_with_icon(
                        "switch-screen",
                        if compact { "Trocar" } else { "Trocar tela" },
                        "icons/refresh.svg",
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.choose_source(cx))),
                )
                .child(
                    button_with_colored_icon(
                        "stop",
                        if compact {
                            "Parar tela"
                        } else {
                            "Parar de transmitir"
                        },
                        "icons/stop.svg",
                        false,
                        LIVE,
                    )
                    .border_color(gpui::rgba(0xF0443A70))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let _ = this.backend.commands.send(Command::StopSharing);
                        cx.notify();
                    })),
                );
        } else {
            row = row.child(
                button_with_icon("share", "Compartilhar tela", "icons/share.svg", true)
                    .on_click(cx.listener(|this, _, _, cx| this.choose_source(cx))),
            );
        }
        row.child(
            quiet_button("leave", "Sair")
                .on_click(cx.listener(|this, _, window, cx| this.leave(window, cx))),
        )
        .into_any_element()
    }
}

fn empty_state(path: &'static str, title: &'static str, description: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_4()
        .p_8()
        .child(
            div()
                .size(px(64.))
                .rounded(px(16.))
                .bg(rgb(BG))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(path, 30., MUTED)),
        )
        .child(
            div()
                .mt_2()
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_color(rgb(MUTED))
                .text_center()
                .child(description),
        )
}
