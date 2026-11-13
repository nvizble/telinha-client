use super::*;
use livekit::webrtc::desktop_capturer::DesktopCaptureSourceType as Kind;

impl Telinha {
    pub(super) fn picker(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let columns = if window.viewport_size().width >= px(1040.) {
            3
        } else {
            2
        };
        let title = div()
            .flex_shrink_0()
            .px_6()
            .pt_3()
            .pb_5()
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if self.publishing.is_some() {
                                "Qual tela você quer transmitir agora?"
                            } else {
                                "O que você quer compartilhar?"
                            }),
                    )
                    .child(div().text_sm().text_color(rgb(MUTED)).child(
                        if cfg!(target_os = "macos") {
                            "Escolha a qualidade. O macOS cuida da seleção da janela ou tela."
                        } else {
                            "Escolha uma janela ou uma tela para mostrar aos seus amigos."
                        },
                    )),
            )
            .when(!cfg!(target_os = "macos"), |title| {
                title.child(
                    button_with_icon("refresh", "Atualizar", "icons/refresh.svg", false)
                        .when(self.starting || self.loading_sources, |button| {
                            button.opacity(0.45)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.starting && !this.loading_sources {
                                this.choose_source(cx);
                            }
                        })),
                )
            });
        let mut sources = div()
            .id("source-list")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .px_6()
            .pb_6()
            .flex()
            .flex_col()
            .gap_6();
        if self.loading_sources {
            sources = sources.child(
                div()
                    .p_8()
                    .rounded(px(10.))
                    .bg(rgb(SUNK))
                    .text_color(rgb(MUTED))
                    .child("Procurando janelas e telas…"),
            );
        } else if cfg!(target_os = "macos") && !self.sources.is_empty() {
            sources = sources.child(div().rounded(px(12.)).border_1().border_color(rgb(LINE)).bg(rgb(SUNK))
                .p_8().flex().flex_col().items_center().gap_5()
                .child(div().w(px(180.)).h(px(116.)).rounded(px(12.)).bg(rgb(BG)).flex().items_center().justify_center()
                    .child(icon("icons/window.svg", 64., FG)))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child("Sua janela, sua escolha"))
                .child(div().max_w(px(360.)).text_sm().line_height(px(22.)).text_center().text_color(rgb(MUTED))
                    .child("Ao começar, escolha uma janela ou tela no seletor do macOS. Só o conteúdo que você escolher será compartilhado.")));
        } else if self.sources.is_empty() {
            sources = sources.child(
                div()
                    .p_8()
                    .text_color(rgb(MUTED))
                    .child("Nenhuma janela disponível. Abra um programa e atualize a lista."),
            );
        } else {
            for (kind, label, note) in [
                (Kind::Window, "Janelas", "Compartilhe apenas um aplicativo"),
                (
                    Kind::Screen,
                    "Telas",
                    "Tudo o que aparecer na tela será compartilhado",
                ),
            ] {
                let choices: Vec<_> = self
                    .sources
                    .iter()
                    .enumerate()
                    .filter(|(_, source)| source.kind == kind)
                    .collect();
                if choices.is_empty() {
                    continue;
                }
                let mut grid = div().flex_shrink_0().grid().grid_cols(columns).gap_3();
                for (index, source) in choices {
                    let selected = self.selected_source == Some(index);
                    let source_id = source.id;
                    let path = if kind == Kind::Screen {
                        "icons/monitor.svg"
                    } else {
                        "icons/window.svg"
                    };
                    grid = grid.child(
                        div()
                            .id(("source", index))
                            .min_w_0()
                            .h(px(196.))
                            .min_h(px(196.))
                            .flex()
                            .flex_col()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(rgb(if selected { FG } else { LINE }))
                            .bg(rgb(if selected { PANEL } else { SUNK }))
                            .cursor_pointer()
                            .overflow_hidden()
                            .hover(|style| style.bg(rgb(PANEL)))
                            .when(self.starting, |card| card.opacity(0.55).cursor_default())
                            .child(
                                div()
                                    .relative()
                                    .h(px(126.))
                                    .flex_shrink_0()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .items_center()
                                    .justify_center()
                                    .when_some(
                                        self.source_previews.get(&index),
                                        |preview, image| {
                                            preview.child(
                                                img(image.clone())
                                                    .size_full()
                                                    .object_fit(ObjectFit::Contain),
                                            )
                                        },
                                    )
                                    .when(!self.source_previews.contains_key(&index), |preview| {
                                        preview
                                            .child(icon(
                                                path,
                                                30.,
                                                if selected { FG } else { MUTED },
                                            ))
                                            .child(div().text_xs().text_color(rgb(MUTED)).child(
                                                if self.source_preview_done.contains(&index) {
                                                    "Prévia indisponível"
                                                } else {
                                                    "Carregando prévia…"
                                                },
                                            ))
                                    })
                                    .when(selected, |preview| {
                                        preview.child(
                                            div()
                                                .absolute()
                                                .right_2()
                                                .top_2()
                                                .size(px(20.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_full()
                                                .bg(rgb(FG))
                                                .child(icon("icons/check.svg", 14., SUNK)),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .min_w_0()
                                    .px_3()
                                    .py_3()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate()
                                            .child(source.name.clone()),
                                    )
                                    .child(div().text_xs().text_color(rgb(MUTED)).child(
                                        if cfg!(target_os = "windows") {
                                            "Áudio selecionado à parte"
                                        } else {
                                            "Sem áudio"
                                        },
                                    )),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.starting && !this.loading_sources {
                                    this.selected_source = this.sources.iter().position(|source| {
                                        source.id == source_id && source.kind == kind
                                    });
                                    this.update_audio_for_source();
                                    cx.notify();
                                }
                            })),
                    );
                }
                sources = sources.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(label),
                                )
                                .child(div().text_xs().text_color(rgb(MUTED)).child(note)),
                        )
                        .child(grid),
                );
            }
        }
        let mut options = div()
            .id("sharing-options")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p_5()
            .flex()
            .flex_col()
            .gap_5();
        if cfg!(target_os = "windows") {
            options = options.child(self.audio_options(cx));
        } else {
            options = options.child(div().w_full().min_w_0().flex_shrink_0().flex().items_start().gap_3().text_color(rgb(MUTED))
                .child(div().flex_shrink_0().child(icon("icons/mute.svg", 18., MUTED)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1().whitespace_normal()
                    .child(div().w_full().text_sm().line_height(px(20.)).child("Compartilhamento sem áudio"))
                    .child(div().w_full().text_xs().line_height(px(18.)).child(if cfg!(target_os = "macos") {
                        "Esta versão do Mac compartilha vídeo. Você continua ouvindo as transmissões recebidas."
                    } else { "Para enviar som, escolha a janela do aplicativo." }))));
        }
        let mut quality_choices = div().flex_shrink_0().flex().flex_col().gap_2().child(
            div()
                .mb_1()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Qualidade"),
        );
        for (index, (quality, note)) in [
            (Quality::FullHd60, "Mais fluidez"),
            (Quality::FullHd30, "Menos uso do PC"),
            (Quality::Hd60, "Menos banda"),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = self.quality == quality;
            quality_choices = quality_choices.child(
                div()
                    .id(("quality", index))
                    .flex_shrink_0()
                    .p_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if selected { FG } else { LINE }))
                    .bg(rgb(if selected { PANEL } else { BG }))
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL)))
                    .when(self.starting || self.publishing.is_some(), |option| {
                        option.opacity(0.45)
                    })
                    .child(
                        div()
                            .size(px(16.))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(if selected { FG } else { MUTED }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |circle| {
                                circle.child(div().size(px(8.)).rounded_full().bg(rgb(FG)))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().child(quality.label()))
                            .child(div().text_xs().text_color(rgb(MUTED)).child(note)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.starting && this.publishing.is_none() {
                            this.quality = quality;
                            cx.notify();
                        }
                    })),
            );
        }
        options = options.child(quality_choices);
        let can_start = !self.starting
            && !self.loading_sources
            && self.selected_source.is_some()
            && self.audio_selection_ready();
        let actions = div()
            .flex_shrink_0()
            .px_5()
            .pb_5()
            .pt_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                button_with_icon(
                    "start",
                    if self.starting {
                        "Preparando…"
                    } else if self.publishing.is_some() {
                        "Trocar tela"
                    } else {
                        "Começar a transmitir"
                    },
                    "icons/share.svg",
                    true,
                )
                .when(!can_start, |button| button.opacity(0.45))
                .on_click(cx.listener(|this, _, _, cx| this.share(cx))),
            )
            .child(
                quiet_button("cancel-picker", "Cancelar").on_click(cx.listener(
                    |this, _, _, cx| {
                        if this.starting {
                            let _ = this.backend.commands.send(Command::CancelShare);
                            this.starting = false;
                        }
                        this.close_source_picker(cx);
                        cx.notify();
                    },
                )),
            );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(title)
            .child(
                div().flex_1().min_h_0().flex().child(sources).child(
                    div()
                        .w(px(280.))
                        .flex_shrink_0()
                        .min_h_0()
                        .border_l_1()
                        .border_color(rgb(LINE))
                        .flex()
                        .flex_col()
                        .child(options)
                        .child(actions),
                ),
            )
            .into_any_element()
    }

    fn audio_options(&self, cx: &mut Context<Self>) -> AnyElement {
        use audio_selection::Selection;

        let disabled = self.starting || self.loading_sources;
        let can_follow = self
            .selected_source
            .and_then(|i| self.sources.get(i))
            .is_some_and(|source| {
                source.supports_audio()
                    && self
                        .audio_sources
                        .iter()
                        .any(|app| Some(app.window_id) == source.id)
            });
        let mut audio = div()
            .flex_shrink_0()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .mb_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Fonte de áudio"),
            );
        for (index, label) in [
            "Sem áudio",
            "Seguir janela compartilhada",
            "Outro aplicativo",
        ]
        .into_iter()
        .enumerate()
        {
            let selected = matches!(
                (index, self.audio_selection),
                (0, Selection::Off) | (1, Selection::FollowWindow) | (2, Selection::Application(_))
            );
            audio = audio.child(
                div()
                    .id(("audio-mode", index))
                    .flex_shrink_0()
                    .min_w_0()
                    .p_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if selected { FG } else { LINE }))
                    .bg(rgb(if selected { PANEL } else { BG }))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL)))
                    .when(disabled || (index == 1 && !can_follow), |row| {
                        row.opacity(0.45).cursor_default()
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(14.))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(if selected { FG } else { MUTED }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |circle| {
                                circle.child(div().size(px(6.)).rounded_full().bg(rgb(FG)))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .line_height(px(19.))
                            .whitespace_normal()
                            .child(label),
                    )
                    .when(index == 2, |row| {
                        row.child(div().flex_shrink_0().child(icon(
                            "icons/chevron.svg",
                            12.,
                            MUTED,
                        )))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.starting || this.loading_sources || (index == 1 && !can_follow) {
                            return;
                        }
                        this.audio_selection_warning = None;
                        match index {
                            0 => {
                                this.audio_selection = Selection::Off;
                                this.audio_apps_open = false;
                            }
                            1 => {
                                this.audio_selection = Selection::FollowWindow;
                                this.audio_apps_open = false;
                            }
                            _ => {
                                if matches!(this.audio_selection, Selection::Application(_)) {
                                    this.audio_apps_open = !this.audio_apps_open;
                                } else {
                                    this.audio_selection = Selection::Application(None);
                                    this.audio_apps_open = true;
                                }
                            }
                        }
                        cx.notify();
                    })),
            );
        }

        if matches!(self.audio_selection, Selection::Application(_)) && self.audio_apps_open {
            let mut applications = div()
                .id("audio-applications")
                .flex_shrink_0()
                .min_w_0()
                .max_h(px(160.))
                .overflow_y_scroll()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(LINE))
                .bg(rgb(SUNK))
                .p_1()
                .flex()
                .flex_col();
            if self.audio_sources.is_empty() {
                applications = applications.child(
                    div()
                        .p_2()
                        .text_xs()
                        .line_height(px(18.))
                        .text_color(rgb(MUTED))
                        .whitespace_normal()
                        .child("Abra o aplicativo e atualize a lista."),
                );
            }
            for (index, application) in self.audio_sources.iter().enumerate() {
                let key = audio_application_key(application);
                let selected = self.audio_selection == Selection::Application(Some(key));
                applications = applications.child(
                    div()
                        .id(("audio-application", index))
                        .flex_shrink_0()
                        .min_w_0()
                        .px_2()
                        .py_2()
                        .rounded(px(5.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(PANEL)))
                        .when(selected, |row| row.bg(rgb(PANEL)))
                        .when(disabled, |row| row.opacity(0.45).cursor_default())
                        .child(
                            div()
                                .flex_shrink_0()
                                .child(icon("icons/window.svg", 14., MUTED)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .truncate()
                                .child(application.name.clone()),
                        )
                        .when(selected, |row| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .child(icon("icons/check.svg", 12., FG)),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.starting
                                && !this.loading_sources
                                && this
                                    .audio_sources
                                    .iter()
                                    .any(|application| audio_application_key(application) == key)
                            {
                                this.audio_selection = Selection::Application(Some(key));
                                this.audio_apps_open = false;
                                this.audio_selection_warning = None;
                                cx.notify();
                            }
                        })),
                );
            }
            audio = audio.child(applications);
        }

        if let Some(application) = self.selected_audio_application() {
            audio = audio.child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(format!("Som de {}", application.name)),
            );
        }
        let description = if self.loading_sources {
            "Atualizando aplicativos…"
        } else {
            match self.audio_selection {
                Selection::Off => "Só a imagem será transmitida.",
                Selection::FollowWindow if self.selected_audio_application().is_some() => {
                    "O som acompanha o aplicativo da janela escolhida."
                }
                Selection::FollowWindow => {
                    "Sem áudio para esta imagem. Escolha Outro aplicativo para enviar som."
                }
                Selection::Application(_) if self.selected_audio_application().is_some() => {
                    "Este som será mantido ao escolher outra imagem."
                }
                Selection::Application(_) => "Escolha o aplicativo que seus amigos vão ouvir.",
            }
        };
        audio = audio.child(
            div()
                .min_w_0()
                .text_xs()
                .line_height(px(18.))
                .whitespace_normal()
                .text_color(rgb(MUTED))
                .child(description),
        );
        if let Some(warning) = self.audio_selection_warning {
            audio = audio.child(
                div()
                    .min_w_0()
                    .text_xs()
                    .line_height(px(18.))
                    .whitespace_normal()
                    .text_color(rgb(WARN))
                    .child(warning),
            );
        }
        audio
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .line_height(px(18.))
                    .whitespace_normal()
                    .text_color(rgb(MUTED))
                    .child("Discord e Telinha ficam fora das fontes de áudio para evitar eco."),
            )
            .into_any_element()
    }
}
