use super::*;

impl Telinha {
    pub(super) fn home(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let width = f32::from(window.viewport_size().width);
        let padding = if width < 1040. { 40. } else { 80. };
        let logo_size = if width < 1040. { 160. } else { 192. };
        let can_join = !self.auth_busy && RoomCode::from_input(&self.input).is_ok();
        let mut content = div().flex().flex_col().gap_6().max_w(px(580.));
        if self.account.is_some() {
            content = content.child(self.call_panel(cx));
        } else {
            content = content
                .child(heading("Sua call do Discord vira a sessão"))
                .child(div().text_size(px(16.)).line_height(px(25.)).text_color(rgb(MUTED))
                    .child("Entre com o Discord uma vez. Depois é só entrar numa call: quem estiver nela e abrir a telinha cai na mesma sessão."))
                .child(div().pt_6().flex().child(
                    discord_button(if self.auth_busy { "Conectando sua conta…" } else { "Entrar com o Discord" })
                        .when(self.auth_busy, |button| button.opacity(0.65).cursor_default())
                        .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                ));
        }
        let footer = div()
            .h(px(112.))
            .flex_shrink_0()
            .px(px(padding))
            .border_t_1()
            .border_color(rgb(LINE))
            .flex()
            .items_center()
            .gap_6()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("Entrar com código ou link"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.code_boxes(true, cx))
                            .child(
                                quiet_button("paste", "Colar")
                                    .ml_2()
                                    .on_click(cx.listener(|this, _, _, cx| this.paste(cx))),
                            )
                            .child(
                                button("join", "Entrar", false)
                                    .when(!can_join, |button| button.opacity(0.45).cursor_default())
                                    .when(can_join, |button| {
                                        button.on_click(cx.listener(|this, _, _, cx| this.join(cx)))
                                    }),
                            ),
                    ),
            )
            .child(div().flex_1())
            .child(
                button("create", "Criar sessão", false)
                    .when(self.auth_busy, |button| button.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.connect(RoomCode::generate(), cx))),
            );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("home-main")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(padding))
                    .py_8()
                    .flex()
                    .items_center()
                    .gap(px(56.))
                    .child(div().flex_1().min_w_0().my_auto().child(content))
                    .child(
                        div()
                            .w(px(logo_size + 24.))
                            .flex_shrink_0()
                            .my_auto()
                            .flex()
                            .justify_center()
                            .child(crate::home_logo::home_logo(logo_size)),
                    ),
            )
            .child(footer)
            .into_any_element()
    }

    fn call_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let (title, subtitle) = if let Some(error) = &self.call_error {
            ("Não encontramos sua call".to_owned(), error.clone())
        } else {
            match &self.call_status {
                Some(CallStatus::InCall { call }) => (
                    format!("# {}", call.channel_name),
                    format!("{} · sua sessão está pronta para abrir.", call.guild_name),
                ),
                Some(CallStatus::Multiple { .. }) => ("Escolha sua call".into(), "Você está em mais de um canal de voz. Escolha qual sessão quer abrir.".into()),
                Some(CallStatus::Waiting) => ("Entre numa call do Discord".into(), "A sessão abre sozinha quando você entrar num canal de voz.".into()),
                Some(CallStatus::Unavailable { reason: CallUnavailable::NotInstalled }) => ("Leve a telinha para sua call".into(), "Adicione a telinha ao seu servidor para encontrar seus amigos automaticamente.".into()),
                Some(CallStatus::Unavailable { reason: CallUnavailable::Disabled }) => ("Sua tela, com seus amigos".into(), "A entrada pela call está indisponível. Você pode criar uma sessão ou entrar com um código.".into()),
                _ => ("Procurando sua call".into(), "Estamos conectando ao Discord. Sua sessão aparece aqui em instantes.".into()),
            }
        };
        let mut content = div().flex().flex_col().gap_5().child(heading(title)).child(
            div()
                .text_size(px(16.))
                .line_height(px(25.))
                .text_color(rgb(MUTED))
                .child(subtitle),
        );
        match &self.call_status {
            Some(CallStatus::InCall { call }) => {
                let target = call.clone();
                let mut peers = div().flex().items_center().gap_2();
                for peer in call.participants.iter().take(5) {
                    peers =
                        peers.child(self.avatar(Some(&peer.avatar_url), &peer.display_name, 28.));
                }
                content = content
                    .child(
                        peers.child(
                            div()
                                .ml_1()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .child(format!("{} na call", call.participants.len())),
                        ),
                    )
                    .child(
                        div().flex().child(
                            button_with_icon(
                                "join-found-call",
                                "Entrar na call",
                                "icons/arrow.svg",
                                true,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.connect_call(target.clone(), cx),
                            )),
                        ),
                    );
            }
            Some(CallStatus::Multiple { calls }) => {
                let mut choices = div()
                    .id("call-choices")
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2();
                for (index, call) in calls.iter().enumerate() {
                    let target = call.clone();
                    choices =
                        choices.child(
                            button_with_icon(
                                ("choose-call", index),
                                format!("{} · # {}", call.guild_name, call.channel_name),
                                "icons/arrow.svg",
                                false,
                            )
                            .justify_between()
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.connect_call(target.clone(), cx),
                            )),
                        );
                }
                content = content.child(choices);
            }
            _ => {}
        }
        content =
            content.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .bg(rgb(if self.auto_join.enabled { MUTED } else { WARN })),
                    )
                    .child(div().text_sm().text_color(rgb(MUTED)).child(
                        if self.auto_join.enabled {
                            "Entrada automática ativada"
                        } else {
                            "Entrada automática pausada"
                        },
                    ))
                    .child(
                        quiet_button(
                            "auto-join-toggle",
                            if self.auto_join.enabled {
                                "Pausar"
                            } else {
                                "Retomar"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.auto_join.toggle();
                            cx.notify();
                        })),
                    ),
            );
        if matches!(
            &self.call_status,
            Some(
                CallStatus::Waiting
                    | CallStatus::Unavailable {
                        reason: CallUnavailable::NotInstalled
                    }
            )
        ) {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("A telinha precisa estar no servidor da sua call."),
                    )
                    .child(
                        quiet_button("install-discord-bot", "Adicionar a um servidor")
                            .px_0()
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.open_url(&this.backend.account.invite_url)
                            })),
                    ),
            );
        }
        content.into_any_element()
    }
}
