# Componentes de terceiros

A licença restritiva do Telinha se aplica ao código original do projeto.
Não substitui nem reduz permissões concedidas pelos componentes de terceiros.
As versões exatas de todas as dependências estão no `Cargo.lock`; cada pacote
baixado pelo Cargo inclui seus próprios metadados e avisos de licença.

## Código incluído no repositório

### Nokhwa bindings para Windows 0.4.6 — Apache-2.0

Copyright 2022 l1npengtul / The Nokhwa Contributors.

- Origem: <https://github.com/l1npengtul/nokhwa>
- Licença integral: [vendor/nokhwa-bindings-windows/LICENSE](vendor/nokhwa-bindings-windows/LICENSE)
- Alterações locais: [vendor/nokhwa-bindings-windows/TELINHA.md](vendor/nokhwa-bindings-windows/TELINHA.md)

O código vendorizado e as correções locais nele continuam sob Apache-2.0.
Os avisos de autoria no código original foram preservados.

### Pictogramas

Os traços de ícones em `app/src/icons.rs` incluem formas usadas pelos projetos
Feather e Lucide. As respectivas permissões e atribuições são preservadas:

- Feather, Copyright (c) 2013–2023 Cole Bemis — [MIT](licenses/Feather-MIT.txt).
- Lucide Icons and Contributors — [ISC e MIT para ícones derivados do Feather](licenses/Lucide-ISC.txt).

Esses termos se aplicam aos desenhos correspondentes, independentemente da
licença do código que os exibe. A marca e a logo do Telinha são assets próprios.

## Principais dependências obtidas durante o build

| Componente | Versão | Licença declarada | Origem |
| --- | --- | --- | --- |
| GPUI | 0.2.2 | Apache-2.0 | <https://crates.io/crates/gpui/0.2.2> |
| LiveKit | 0.10.1 | Apache-2.0 | <https://github.com/livekit/rust-sdks> |
| libwebrtc | 0.4.0 | Apache-2.0 | <https://github.com/livekit/rust-sdks> |
| webrtc-sys | 0.3.53 | Apache-2.0 | <https://github.com/livekit/rust-sdks> |
| Nokhwa | 0.10.11 | Apache-2.0 | <https://github.com/l1npengtul/nokhwa> |
| WASAPI | 0.27.0 | MIT | <https://crates.io/crates/wasapi/0.27.0> |
| RustCrypto p256 | 0.13.2 | Apache-2.0 OR MIT | <https://github.com/RustCrypto/elliptic-curves> |
| zeroize | Cargo.lock | Apache-2.0 OR MIT | <https://github.com/RustCrypto/utils> |

A tabela é um resumo, não uma lista completa das dependências transitivas.
O SDK nativo WebRTC e suas bibliotecas incorporadas mantêm suas licenças e
avisos próprios; o build os obtém através do SDK LiveKit. Os fontes e binários
dessas dependências não são relicenciados por este repositório.
