# SeX — исходная цель проекта

Этот документ фиксирует требования пользователя, а не заявляет, что они уже
реализованы. Проверенные результаты и оставшаяся работа ведутся отдельно в
[project-status.md](project-status.md). Изменение реализации не должно незаметно
ослаблять эти требования.

## Назначение и главные инварианты

SeX (Sound Exchange) — более основательная версия идеи SoX: offline-first
аудиопроцессор, где качество важнее latency и throughput. Ресемплер — центральная
часть проекта; остальные эффекты поддерживают его, а не подменяют приоритеты.

- Canonical sample representation — fixed-point, не float.
- Арифметика детерминированна; одинаковые вход, конфигурация и seed должны
  давать побитно одинаковый результат независимо от CPU.
- Arbitrary precision доступна в тяжёлых режимах. MPFR допустим для расчёта
  коэффициентов, но не заменяет целочисленный основной signal path.
- Никакого молчаливого wrap. Округление, saturation, sign extension,
  guard bits и поведение переполнения определяются явно.
- Offline не означает «загрузить весь трек в RAM»: обработка потоковая,
  блоками, с сохранением FIR state/overlap. Большие блоки разрешены, не обязательны.

## Reference и архитектура

Сначала изучить C-исходники SoX: форматный слой и парсинг, DSP pipeline,
реализацию rate, тестирование, dithering, clipping semantics и metadata.
Не переносить архитектуру вслепую; письменно отделить полезный reference от
того, что переписывается с нуля. Заимствование кода требует соблюдения лицензии.

Целевая декомпозиция:

| Компонент | Ответственность |
| --- | --- |
| libsexio | Форматы, metadata, streaming |
| libsexq | Q arithmetic и backend abstraction |
| libsexfir | Проектирование и квантование FIR |
| libsexrate | Рациональный polyphase resampling engine |
| libsexdsp | Общий DSP pipeline |

CLI — `sex`; экосистема включает `libsex`, `sex-rate`, `sex-analyze`,
`sex-bench`, `sex-fuzz`. Имена выше задают границы ответственности, а не
обязательный язык реализации или уже предоставленный ABI.

## Sample model и арифметика

Поддержать Q1.31, Q1.63, Qm.n с compile/runtime configurable width и
произвольно широкие Q-форматы для тяжёлых пресетов. Чётко определить, где
учитывается знаковый бит, диапазон значений и допустимый headroom.

Выделить операции `q_add`, `q_sub`, `q_mul`, `q_mac`, `q_shift`, `q_round`,
`q_saturate`, `q_rescale`. Реализации: native 32-bit, native 64-bit,
128-битная расширенная арифметика и bigint через GMP/MPIR либо собственную
абстракцию. Правила должны оставаться одинаковыми между backend'ами.

FIR: sample × coefficient даёт расширенное точное произведение; все слагаемые
накапливаются в достаточно широком accumulator. Округление — один раз после
суммы, без ненужных промежуточных округлений. В Until-40k допустим accumulator
8192 бит и шире, если этого требует расчёт.

## Resampling и filter design

- Rate ratios хранить и исполнять как точные дроби: 48000/44100 = 160/147.
  Пользовательские дробные/десятичные rates не превращать в приближённый double.
- Использовать rational polyphase decomposition и считать только необходимые
  фазы, без буквальной материализации upsample ×160 → огромный FIR → /147.
- Отделить designer от engine. Нужны windowed sinc, Kaiser,
  Dolph–Chebyshev, equiripple / Parks–McClellan; least-squares FIR —
  дополнительное желательное направление.
- Designer выдаёт коэффициенты в выбранном Q-format. Quantization-aware
  design оценивает изменение passband ripple и stopband attenuation после
  квантования и при необходимости автоматически увеличивает fractional width.
- Precision planner по ratio и `--error-floor` выбирает fractional bits,
  guard bits, минимальную ширину accumulator, бюджет ошибки коэффициентов
  и длину FIR. Ограничения и невыполнимые запросы объясняются явно.
- Хранить рассчитанные коэффициенты в cache: длинные FIR с тысячами
  fractional bits не должны без необходимости проектироваться заново.

## Пресеты

`fast`, `sane`, `high`, `absurd`, `pointless`, `until-40k` должны означать
конкретные инженерные ограничения: transition width, stopband attenuation,
coefficient precision, accumulator width и dithering policy.
Численные спецификации и доказательства их выполнения важнее названия.

Until-40k — отдельный тяжёлый режим: bigint Q arithmetic, автоматически
увеличиваемая precision, очень длинные FIR и максимально узкая transition
band. Допускаются много проходов и высокая стоимость расчёта. Никаких
SIMD-only компромиссов или скрытого снижения качества ради скорости.

## Pipeline, PCM и форматы

Pipeline API должен позволять gain, channel mix, DC removal, filters,
convolution, dither и format conversion.

Финальное квантование PCM: TPDF, high-pass TPDF, noise shaping,
выбираемая bit depth и детерминированный seeded режим для тестов.

Clipping policies: `saturate`, `error`, `normalize`, `allow-headroom`.
Смысл headroom внутри тракта и ограничения конечного PCM описать отдельно;
silent wrap не разрешать по умолчанию.

Предпочитать libsndfile или аналогичный адаптер для форматов, особенно в
первой версии: разработка WAV/AIFF/FLAC-парсеров не должна вытеснить разработку
ресемплера. Сохранять корректную работу с metadata.

## CLI и измеримость

Поддержать знакомую SoX-подобную форму и расширенные настройки:

```text
sex input.wav -r 48000 output.wav
sex input.wav output.wav rate 48000 --preset until-40k --precision auto --error-floor -400dB
sex in.wav out.wav --rate 48000 --error-floor -300dB
```

`sex analyze` должен измеримо сообщать passband ripple, stopband floor,
coefficient quantization error и accumulator headroom. Иллюстративные числа
из исходного запроса не являются заранее обещанными результатами.

Документация объясняет математическую модель: тип и длину FIR, window,
Q-format, места округления, accumulator и theoretical error bound.
Разделять теоретические границы, измерения на сетке, непрерывные доказательства
и шум конечного PCM; не выдавать одно за другое.

## Валидация и производительность

Сравнивать с SoX, libsoxr и libsamplerate на sweeps, impulses, multitone,
near-Nyquist tones и сложных ratios. Проверять SNR, passband ripple,
stopband rejection, aliasing, impulse response, phase response и error spectrum.

Обязательные torture cases: DC, full-scale sine, alternating ±FS, impulse,
silence, 1 Hz, тон прямо у Nyquist, необычный rate, сведённый к рациональному,
многоканальный PCM и файл из одного sample. Проверять независимость результата
от разбиения на блоки. «Denormal equivalent» для integer-тракта не нужен.

Большие файлы проверять экономно: сотни гигабайт реального дискового вывода
не являются обязательным тестом и допустимы только при обоснованной необходимости
после устойчивой работы остальных частей.

SIMD добавлять после core correctness, исключительно как ускорение того же
exact/fixed algorithm. AVX/SSE и другие backend'ы не должны менять результат.

## Порядок реализации и критерий завершения

Первый milestone: Q1.63 + 128-битный accumulator + polyphase FIR + WAV
input/output + streaming + бит-точные тесты. Не начинать с bigint;
сначала подтвердить корректность базового тракта, затем расширять precision
и открывать Until-40k.

Далее развивать designers, quantization-aware planner, пресеты и analyze;
pipeline, dither, clipping, cache и воспроизводимость; bigint/MPFR и тяжёлые
режимы; SIMD, benchmarks, fuzz и инструменты экосистемы.

Цель завершена только при наличии проверенного, измеримого тракта и
документированных математических спецификаций. Частичный milestone, успешный
smoke test или несколько совпавших файлов не означают готовность всей цели.
