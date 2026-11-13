# Telinha — cliente desktop

Cliente nativo de compartilhamento de tela e webcam para Windows e macOS,
feito em Rust com GPUI e LiveKit. Várias pessoas podem transmitir ao mesmo
tempo; a conversa por voz continua no Discord.

**Código disponível para consulta e compilação pessoal (source-available).**
A [licença](LICENSE) permite consultar, compilar e executar para uso próprio
não comercial. Não permite redistribuir o código ou os binários, nem explorá-los
comercialmente, ressalvados os direitos de terceiros e os direitos de plataforma
descritos na licença. Não é uma licença open source aprovada pela OSI.

[Site e download oficial](https://telinha.tech) ·
[Verificações de build](https://github.com/nvizble/telinha-client/actions)

## Conteúdo

- `app/`: interface, captura, reprodução, conta, atualizações e feedback.
- `shared/`: contratos da API, validações e verificação de atualizações.
- `vendor/nokhwa-bindings-windows/`: correção local do suporte a webcams no Windows.
- `scripts/`: compilação e empacotamento local.

Este repositório compila sozinho. O servidor, o bot Discord, o viewer web, o
site e o painel administrativo são mantidos separadamente. O app se conecta a
`https://telinha.tech` por padrão; nenhum segredo de infraestrutura é necessário
para compilar. As funções online dependem da disponibilidade desse serviço.

## Compilar no macOS

Requisitos: macOS 14 ou posterior, Git, Rust instalado com rustup, Python 3.11+
e Xcode Command Line Tools (`xcode-select --install`). O toolchain Rust está
fixado em `rust-toolchain.toml` e é instalado automaticamente pelo rustup.
Os shaders do GPUI compilam em runtime, sem exigir o Xcode completo.

```sh
git clone https://github.com/nvizble/telinha-client.git
cd telinha-client
python3 scripts/bundle_macos.py
open dist/telinha.app
```

Para otimização, use `python3 scripts/bundle_macos.py --release`. O script compila
também o auxiliar Swift da câmera, inclui os ícones e as descrições de permissão,
assina o pacote localmente e registra o protocolo `telinha://`. O pacote usa
assinatura ad hoc; não é notarizado. Use o `.app` para ter o auxiliar de webcam.
O macOS solicita acesso à tela/câmera quando você usa esses recursos.

## Compilar no Windows

Requisitos: Windows x64, Git, Rust com toolchain MSVC e Visual Studio Build Tools
com **Desktop development with C++** e Windows SDK, incluindo `fxc.exe`.
Execute no PowerShell:

```powershell
git clone https://github.com/nvizble/telinha-client.git
cd telinha-client
rustup target add x86_64-pc-windows-msvc
./scripts/build-windows.ps1
./dist/telinha.exe
```

O executável fica em `dist/telinha.exe`, acompanhado do SHA-256. Para um build de
desenvolvimento mais rápido, acrescente `-DebugBuild`. O script localiza o `fxc.exe`
do SDK; se necessário, informe seu caminho pela variável `GPUI_FXC_PATH`.
O áudio por aplicativo exige Windows build 20348 ou posterior, como Windows 11.

No primeiro build, o Cargo baixa dependências e o SDK nativo WebRTC. Reserve
vários GB de disco, conexão com a internet e alguns minutos para a compilação.

## Verificar

Em uma das plataformas desktop suportadas:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
```

Após empacotar no Mac:

```sh
dist/telinha.app/Contents/MacOS/telinha --check-fonts
dist/telinha.app/Contents/MacOS/telinha --camera-probe
```

No Windows, `./dist/telinha.exe --camera-probe` também está disponível e já é
executado pelo script de build. Esse teste usa imagens sintéticas, sem acessar
a webcam, a tela ou o microfone. Os workflows públicos verificam builds e testes
nas duas plataformas; não contêm credenciais de produção ou de assinatura.

## Serviço e dados

Tela, webcam e áudio usam E2EE AES-GCM do LiveKit. Chaves são geradas nos dispositivos e trocadas automaticamente em envelopes ECDH P-256/HKDF/AES-GCM. O relay não recebe chaves de mídia em claro; não há fallback sem criptografia. Entrada automática e links públicos continuam funcionando: participantes admitidos recebem as chaves e podem gravar a mídia.

A autenticação das chaves e a lista de membros dependem da API de admissão. Não há verificação fora de banda; um servidor de admissão malicioso poderia substituir chaves ou admitir participantes. E2EE não oculta metadados nem protege dispositivos comprometidos. `shared/testdata/e2ee-v1.json` contém apenas chaves artificiais públicas para teste.

**Trocar tela** preserva a sessão e a webcam. No Windows, compartilhar um monitor pede a fonte de áudio explicitamente; Discord e Telinha não são fontes permitidas. Se o Discord estiver em uma aba, escolha o jogo como fonte: captura por processo não separa abas de navegador.

O cliente envia os dados necessários para entrar na sessão, autenticar a conta
opcional do Discord e publicar/receber mídia. Nome e avatar identificam os
participantes. A webcam começa desligada e é ativada pela escolha do usuário.

O serviço registra eventos de sessão e de publicação de mídia a partir dos
webhooks autenticados do LiveKit, como entrada/saída e início/fim de webcam ou
tela. Esse registro contém metadados, não quadros do vídeo. Não existe uma chamada
separada de analytics do cliente para registrar “webcam ligada”. O código e os
dados desse registro administrativo não fazem parte deste repositório.

O feedback é opcional: notas e comentário só são enviados ao confirmar o
formulário e ficam associados à pessoa e à sessão, como informado na interface.
Logs de diagnóstico de captura são locais. Não publique tokens, códigos de salas
privadas ou dados pessoais ao relatar um problema.

## Atualizações e builds pessoais

O Windows mantém o atualizador da distribuição oficial: verifica versões,
baixa uma atualização assinada e oferece **Reiniciar e atualizar**. Aceitar essa
oferta substitui o build pessoal pela versão oficial. No macOS, recompile para
atualizar. `shared/assets/update-key.bin` é somente a chave **pública** de
verificação Ed25519, e não permite assinar atualizações.

`TELINHA_SERVER_URL` permite selecionar uma API compatível para desenvolvimento.
A API e as ferramentas para operar o serviço não são distribuídas aqui. Os
testes de mídia que dependem de um backend não são executados contra produção
pelos workflows públicos.

## Licenças e manutenção

Copyright © 2026 Athenabyte S T Ltda.

Leia [LICENSE](LICENSE) para os termos completos e
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) para as exceções de terceiros.
GitHub permite visualizar e criar forks dentro da plataforma conforme seus
[termos](https://docs.github.com/en/site-policy/github-terms/github-terms-of-service#d-user-generated-content);
isso não transforma o projeto em software livre.

O histórico público começa com um snapshot do cliente. `SOURCE.json` identifica
a versão e os hashes dos arquivos desse snapshot. Novas versões são sincronizadas
pela manutenção do Telinha. Relatos de bugs são bem-vindos nas Issues; neste
momento, não estamos solicitando contribuições de código sob uma licença aberta.
