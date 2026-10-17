/*
 * Copyright (c) 2026 Ankit Chaubey <ankitchaubey.dev@gmail.com>
 * https://github.com/ankit-chaubey
 *
 * Project: ferogram
 * Website: https://ferogram.dev
 *
 * Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
 * https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
 * <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
 * This file may not be copied, modified, or distributed except according
 * to those terms.
 */

use ferogram_tl_types as tl;

// Button (inline keyboard buttons)

/// A single inline keyboard button.
///
/// Layer 229 split what used to be one `KeyboardButton` schema into two:
/// `KeyboardButton`/`ButtonType` for reply keyboards, and
/// `KeyboardInlineButton`/`InlineButtonType` for inline keyboards. This
/// type only builds the latter - for reply-keyboard buttons, see
/// [`ReplyButton`].
#[derive(Clone)]
pub struct Button {
    text: String,
    style: Option<tl::enums::KeyboardButtonStyle>,
    kind: tl::enums::InlineButtonType,
}

impl Button {
    /// A button that sends a callback data payload when pressed.
    pub fn callback(text: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::Callback(tl::types::InlineButtonTypeCallback {
                requires_password: false,
                data: data.into(),
            }),
        }
    }

    /// A button that opens a URL in the browser.
    pub fn url(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::Url(tl::types::InlineButtonTypeUrl {
                url: url.into(),
            }),
        }
    }

    /// A button that opens a user-profile or bot link in Telegram.
    ///
    /// `bot` became optional in layer 229 (it was required before); passed
    /// through as `Some` here to keep this constructor's signature the same
    /// as before the upgrade.
    pub fn url_auth(
        text: impl Into<String>,
        url: impl Into<String>,
        fwd_text: Option<String>,
        bot: tl::enums::InputUser,
    ) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::InputInlineButtonTypeUrlAuth(
                tl::types::InputInlineButtonTypeUrlAuth {
                    request_write_access: false,
                    fwd_text,
                    url: url.into(),
                    bot: Some(bot),
                },
            ),
        }
    }

    /// A button that switches to inline mode in the current chat.
    pub fn switch_inline(text: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::SwitchInline(
                tl::types::InlineButtonTypeSwitchInline {
                    same_peer: true,
                    query: query.into(),
                    peer_types: None,
                },
            ),
        }
    }

    /// A button that switches to inline mode in a different (user-chosen) chat.
    pub fn switch_elsewhere(text: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::SwitchInline(
                tl::types::InlineButtonTypeSwitchInline {
                    same_peer: false,
                    query: query.into(),
                    peer_types: None,
                },
            ),
        }
    }

    /// A button that opens a mini-app (full WebView with JS bridge).
    pub fn mini_app(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::WebView(tl::types::InlineButtonTypeWebView {
                url: url.into(),
            }),
        }
    }

    // mini_app_simple() removed here: layer 229's `InlineButtonType` has no
    // "simple webview" variant - `buttonTypeSimpleWebView` only exists
    // under `ButtonType` now (reply keyboards). See
    // [`ReplyButton::mini_app_simple`] for that. This is a real capability
    // change from Telegram, not something ferogram is choosing to drop.

    /// A button that launches a game (bots only).
    pub fn game(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::Game,
        }
    }

    /// A buy button for payments (bots only).
    pub fn buy(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::Buy,
        }
    }

    /// A copy-to-clipboard button.
    pub fn copy_text(text: impl Into<String>, copy_text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::InlineButtonType::Copy(tl::types::InlineButtonTypeCopy {
                copy_text: copy_text.into(),
            }),
        }
    }

    /// Attach a `keyboardButtonStyle#4fdd3430` colour/icon to this button.
    ///
    /// Inline buttons render with background colours chosen by the bot
    /// (`bg_primary`, `bg_danger`, `bg_success`) and an optional custom-emoji
    /// icon. The style travels on `keyboardInlineButton` flag 10, so it
    /// survives edits too.
    ///
    /// ```rust,no_run
    /// use ferogram::keyboard::Button;
    /// let b = Button::callback("Stop", b"stop")
    ///     .style(Button::style_danger());
    /// ```
    pub fn style(mut self, style: tl::enums::KeyboardButtonStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// A `bg_primary` button style (accent colour).
    pub fn style_primary() -> tl::enums::KeyboardButtonStyle {
        tl::enums::KeyboardButtonStyle::KeyboardButtonStyle(tl::types::KeyboardButtonStyle {
            bg_primary: true,
            bg_danger: false,
            bg_success: false,
            icon: None,
        })
    }

    /// A `bg_danger` button style (red).
    pub fn style_danger() -> tl::enums::KeyboardButtonStyle {
        tl::enums::KeyboardButtonStyle::KeyboardButtonStyle(tl::types::KeyboardButtonStyle {
            bg_primary: false,
            bg_danger: true,
            bg_success: false,
            icon: None,
        })
    }

    /// A `bg_success` button style (green).
    pub fn style_success() -> tl::enums::KeyboardButtonStyle {
        tl::enums::KeyboardButtonStyle::KeyboardButtonStyle(tl::types::KeyboardButtonStyle {
            bg_primary: false,
            bg_danger: false,
            bg_success: true,
            icon: None,
        })
    }

    /// Consume into the raw TL type.
    pub fn into_raw(self) -> tl::enums::KeyboardInlineButton {
        tl::enums::KeyboardInlineButton::KeyboardInlineButton(tl::types::KeyboardInlineButton {
            style: self.style,
            text: self.text,
            r#type: self.kind,
        })
    }
}

// ReplyButton (reply keyboard buttons)

/// A single reply-keyboard button (shown below the message input box, not
/// inline). For inline keyboard buttons, see [`Button`].
#[derive(Clone)]
pub struct ReplyButton {
    text: String,
    style: Option<tl::enums::KeyboardButtonStyle>,
    kind: tl::enums::ButtonType,
}

impl ReplyButton {
    /// A plain text button.
    pub fn text(label: impl Into<String>) -> Self {
        Self {
            text: label.into(),
            style: None,
            kind: tl::enums::ButtonType::Default,
        }
    }

    /// A button that requests the user's phone number.
    pub fn request_phone(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::ButtonType::RequestPhone,
        }
    }

    /// A button that requests the user's location.
    pub fn request_geo(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::ButtonType::RequestGeoLocation,
        }
    }

    /// A button that requests the user to create/share a poll.
    pub fn request_poll(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::ButtonType::RequestPoll(tl::types::ButtonTypeRequestPoll {
                quiz: None,
            }),
        }
    }

    /// A button that requests the user to create/share a quiz.
    pub fn request_quiz(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::ButtonType::RequestPoll(tl::types::ButtonTypeRequestPoll {
                quiz: Some(true),
            }),
        }
    }

    /// A button that opens a simple mini-app (no JS bridge, no query_id).
    ///
    /// As of layer 229 this only exists for reply keyboards - see the note
    /// on [`Button`] about why it's not on the inline button builder
    /// anymore. For an inline mini-app with the JS bridge, use
    /// [`Button::mini_app`].
    pub fn mini_app_simple(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            kind: tl::enums::ButtonType::SimpleWebView(tl::types::ButtonTypeSimpleWebView {
                url: url.into(),
            }),
        }
    }

    /// Attach a `keyboardButtonStyle#4fdd3430` colour/icon to this button.
    pub fn style(mut self, style: tl::enums::KeyboardButtonStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Consume into the raw TL type.
    pub fn into_raw(self) -> tl::enums::KeyboardButton {
        tl::enums::KeyboardButton::KeyboardButton(tl::types::KeyboardButton {
            style: self.style,
            text: self.text,
            r#type: self.kind,
        })
    }
}

// InlineKeyboard

/// Builder for an inline keyboard reply markup.
///
/// Each call to [`row`](InlineKeyboard::row) adds a new horizontal row of
/// buttons. Rows are displayed top-to-bottom.
///
/// # Example
/// ```rust,no_run
/// use ferogram::keyboard::{InlineKeyboard, Button};
///
/// let kb = InlineKeyboard::new()
/// .row([Button::callback("Option A", b"a"),
///       Button::callback("Option B", b"b")])
/// .row([Button::url("More info", "https://example.com")]);
/// ```
#[derive(Clone, Default)]
pub struct InlineKeyboard {
    rows: Vec<Vec<Button>>,
}

impl InlineKeyboard {
    /// Create an empty keyboard. Add rows with [`row`](Self::row).
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a row of buttons.
    pub fn row(mut self, buttons: impl IntoIterator<Item = Button>) -> Self {
        self.rows.push(buttons.into_iter().collect());
        self
    }

    /// Convert to the `ReplyMarkup` TL type expected by message-sending functions.
    pub fn into_markup(self) -> tl::enums::ReplyMarkup {
        let rows = self
            .rows
            .into_iter()
            .map(|row| {
                tl::enums::KeyboardInlineButtonRow::KeyboardInlineButtonRow(
                    tl::types::KeyboardInlineButtonRow {
                        buttons: row.into_iter().map(Button::into_raw).collect(),
                    },
                )
            })
            .collect();

        // `force_reply` is a new layer-229 flag on this constructor; not
        // yet exposed via a builder method here, defaulted off to match
        // pre-upgrade behavior.
        tl::enums::ReplyMarkup::ReplyInlineMarkup(tl::types::ReplyInlineMarkup {
            force_reply: false,
            rows,
        })
    }
}

impl From<InlineKeyboard> for tl::enums::ReplyMarkup {
    fn from(kb: InlineKeyboard) -> Self {
        kb.into_markup()
    }
}

// ReplyKeyboard

/// Builder for a reply keyboard (shown below the message input box).
#[derive(Clone, Default)]
pub struct ReplyKeyboard {
    rows: Vec<Vec<ReplyButton>>,
    resize: bool,
    single_use: bool,
    selective: bool,
}

impl ReplyKeyboard {
    /// Create a new empty reply keyboard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a row of text buttons.
    pub fn row(mut self, buttons: impl IntoIterator<Item = ReplyButton>) -> Self {
        self.rows.push(buttons.into_iter().collect());
        self
    }

    /// Resize keyboard to fit its content (recommended).
    pub fn resize(mut self) -> Self {
        self.resize = true;
        self
    }

    /// Hide keyboard after a single press.
    pub fn single_use(mut self) -> Self {
        self.single_use = true;
        self
    }

    /// Show keyboard only to mentioned/replied users.
    pub fn selective(mut self) -> Self {
        self.selective = true;
        self
    }

    /// Convert to `ReplyMarkup`.
    pub fn into_markup(self) -> tl::enums::ReplyMarkup {
        let rows = self
            .rows
            .into_iter()
            .map(|row| {
                tl::enums::KeyboardButtonRow::KeyboardButtonRow(tl::types::KeyboardButtonRow {
                    buttons: row.into_iter().map(ReplyButton::into_raw).collect(),
                })
            })
            .collect();

        // `force_reply` is a new layer-229 flag on this constructor; not
        // yet exposed via a builder method here, defaulted off to match
        // pre-upgrade behavior.
        tl::enums::ReplyMarkup::ReplyKeyboardMarkup(tl::types::ReplyKeyboardMarkup {
            resize: self.resize,
            single_use: self.single_use,
            selective: self.selective,
            persistent: false,
            force_reply: false,
            rows,
            placeholder: None,
        })
    }
}

impl From<ReplyKeyboard> for tl::enums::ReplyMarkup {
    fn from(kb: ReplyKeyboard) -> Self {
        kb.into_markup()
    }
}
