use super::*;

impl Telinha {
    pub(super) fn people_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.participants.len();
        let label = match count {
            0 => "Pessoas".into(),
            1 => "Só você aqui".into(),
            n => format!("{n} pessoas"),
        };
        let mut anchor = div().relative().flex_shrink_0().child(
            div()
                .id("participants-trigger")
                .h(px(36.))
                .px_3()
                .rounded_full()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(rgb(MUTED))
                .cursor_pointer()
                .when(self.participants_open, |button| button.bg(rgb(PANEL)))
                .hover(|style| style.bg(rgb(PANEL)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.account_menu_open = false;
                    this.participants_open = !this.participants_open;
                    cx.notify();
                }))
                .child(icon("icons/people.svg", 16., MUTED))
                .child(label)
                .child(icon("icons/chevron.svg", 12., MUTED)),
        );
        if self.participants_open {
            let current = match &self.call_status {
                Some(CallStatus::InCall { call })
                    if self
                        .room
                        .as_ref()
                        .is_some_and(|room| room.as_str() == call.room) =>
                {
                    Some(call)
                }
                Some(CallStatus::Multiple { calls }) => calls.iter().find(|call| {
                    self.room
                        .as_ref()
                        .is_some_and(|room| room.as_str() == call.room)
                }),
                _ => None,
            };
            let mut list = div()
                .id("session-people-list")
                .max_h(px(360.))
                .overflow_y_scroll()
                .p_2();
            for person in &self.participants {
                let profile = current
                    .and_then(|call| {
                        call.participants
                            .iter()
                            .find(|user| person.id == format!("discord:{}", user.id))
                    })
                    .or(person.profile.as_ref());
                let name = profile
                    .map(|profile| profile.display_name.as_str())
                    .unwrap_or(&person.name);
                let status = match (person.screen, person.camera) {
                    (true, true) => "Tela e webcam ligadas",
                    (true, false) => "Compartilhando tela",
                    (false, true) => "Webcam ligada",
                    (false, false) => "Na sessão",
                };
                let mut title = div().flex().items_center().gap_2().min_w_0().child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(name.to_owned()),
                );
                if person.local {
                    title = title.child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("Você"),
                    );
                }
                let mut info = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(title);
                if let Some(profile) = profile {
                    info = info.child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(format!("@{}", profile.username)),
                    );
                }
                let mut state = div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(rgb(MUTED));
                if person.screen {
                    state = state.child(icon("icons/monitor.svg", 12., LIVE));
                }
                if person.camera {
                    state = state.child(icon("icons/camera.svg", 12., LIVE));
                }
                list = list.child(
                    div()
                        .px_2()
                        .py_3()
                        .flex()
                        .items_start()
                        .gap_3()
                        .child(self.avatar(
                            profile.map(|profile| profile.avatar_url.as_str()),
                            name,
                            34.,
                        ))
                        .child(info.child(state.child(status))),
                );
            }
            if count == 0 {
                list = list.child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("Carregando participantes…"),
                );
            }
            let menu = div()
                .id("participants-menu")
                .absolute()
                .top(px(42.))
                .right_0()
                .w(px(320.))
                .rounded(px(10.))
                .border_1()
                .border_color(rgb(LINE))
                .bg(rgb(SUNK))
                .shadow_lg()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(LINE))
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Pessoas nesta sessão"),
                )
                .child(list);
            anchor = anchor.child(deferred(menu).with_priority(2));
        }
        anchor.into_any_element()
    }
}
