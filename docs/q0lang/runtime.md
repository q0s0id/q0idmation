# q0lang: runtime

Runtime q0lang выполняет AST от parser в изолированное состояние и выдает host
actions. Host сам решает, как применить эти actions.

Текущий runtime лежит здесь:

```text
q0s-format/src/q0lang/runtime.rs
```

## Состояние runtime

Runtime хранит:

- переменные;
- импортированные библиотеки;
- registrations слушателей;
- public function signatures;
- private function signatures;
- timeline actions, которые скрипт сгенерировал при выполнении.

## Значения

Текущие типы значений:

- `Number`;
- `String`;
- `Ident`;
- `Raw`.

`Number` сейчас хранится как floating-point значение, чтобы ядро могло считать
анимационную и геометрическую математику без танцев вокруг целых чисел.

Если assignment получает identifier, runtime пытается взять значение из уже
существующих переменных.

```q0lang
start = 2
copy = start + 3
```

`copy` станет `5`.

Runtime вычисляет арифметику и базовые функции `q0.math`:

```q0lang
import q0.math

x = sin(pi / 2)
y = clamp(lerp(0, 10, 0.75), 2, 6)
name = "Q0" + " tools"
```

Этот слой не требует q0editor: editor только показывает диагностику и summary
состояния, а parser/runtime живут отдельно в `q0s-format`.

## Timeline actions

Runtime не управляет playback напрямую. Он выдает actions:

```text
GoRun(target)
GoStop(target)
```

Targets:

```text
Frame(u16)
Label(String)
```

Так ядро языка можно использовать в разных местах: `q0editor` показывает
summary, а `q0player` применяет actions к playback.

Timeline здесь намеренно остается host action. Математическое ядро языка может
работать без `q0.timeline`, а `q0.timeline` может быть недоступен в CLI, тестах
или future sandbox-режимах.

## Интеграция с q0player

Для v2 `.q0s` файлов q0player сейчас выполняет script у entry-q0rg один раз,
когда файл загружается.

Рабочие примеры:

```q0lang
gostop! 2
```

Фильм загрузится на frame `2` и остановится.

```q0lang
gorun! 3
```

Фильм загрузится на frame `3` и продолжит playback.

Важное ограничение: scripts сейчас хранятся на q0rg-символах, а не на отдельных
кадрах. Поэтому Flash-style frame actions еще не подключены.

## Интеграция с q0editor

q0lang editor показывает:

- diagnostics;
- количество statements;
- built-in libraries;
- Script summary с количеством actions, variables и listener events.

Этот summary использует тот же runtime, что и q0player.

## Ограничения сейчас

- Entry q0rg script выполняется только при загрузке.
- Per-frame script execution еще не реализован.
- Тела функций и вызовы функций еще не реализованы.
- Пользовательские функции пока только регистрируются как signatures; вызовы
  сейчас есть только для встроенной математики.
- Event handlers регистрируются в runtime state, но dispatch mouse events в
  q0player еще не подключен.
- Named timeline labels уже представлены в runtime, но q0player пока разрешает
  только номера кадров и numeric label strings.

## Следующие шаги runtime

1. Добавить script slots на frames или frame actions в project format.
2. Выполнять frame actions при входе playback в кадр.
3. Подключить dispatch `q0.mouse` событий к registered handlers.
4. Добавить тела функций и вызовы функций.
5. Добавить label tables для named timeline targets.
6. Обобщить host actions, чтобы `q0.timeline` был одним модулем эффектов рядом
   с будущими `q0.audio`, `q0.asset`, `q0.debug`.
