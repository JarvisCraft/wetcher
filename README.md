# wetcher

## Системные требования

Для сборки требуется:

* [Rust]

## Установка

```bash
# Создать локальную копию репозитория
git clone git@github.com:JarvisCraft/wetcher.git
# Собрать и установить в систему приложение
cargo install --path=./wetcher --release
```

## Использование

### Получение справки

```bash
wetcher --help
```

```text
Usage: wetcher [OPTIONS]

Options:
  -c, --config <CONFIG>  [default: ./config]
  -h, --help             Print help
  -V, --version          Print version
```

### Запуск системы

```bash
wetcher
```

Опционально можно задать путь до файла конфигурации
с помощью ключа `--config` или `-c`.

## Логирование

Параметры логирования задаются переменной окружения `WETCHER_LOG`,
например `WETCHER_LOG=info` (рекомендуемое значение).

## Конфигурация

Для конфигурации могут использоваться файлы
в форматах JSON, TOML, YAML, INI, RON, JSON5.
По умолчанию название файла -- `config` с соответствующим формату расширением).

### Параметры конфигурации

К корне файла конфигурации содержится ключ `resources`,
в котором перечислены [ресурсы](#Ресурс).

#### Ресурс

Ресурс -- это описание того, как требуется сканировать определённый веб-сайт.

##### `resource`

Конфигурация того, какой веб-сервис требуется сканировать.
Содержит единственное поле `url`, в котором указан URL сайта.

Пример:

```json5
{
  // Требуется просканировать следующий адрес:
  resource: {
    url: "https://progrm-jarvis.ru/misc/java/loom"
  }
}
```

##### `period`

Конфигурация частоты опроса.
Состоит их полей `secs` и `nanos`, соответствующих секундам и наносекундам соответственно.

Пример:

```json5
{
  // Сервис требуется опрашивается раз в 10 минут.
  period: {
    secs: 600,
    nanos: 0,
  }
}
```

##### `targets`

Рекурсивная структура, описывающая правила сканирования ресурсов, например:

```json5
{ // Сканирования с названиями `Product` и `Item`.
  "Product": {
    /* ... конфигурация сканироавния ... */
  },
  "Item": {
    /* ... конфигурация сканироавния ... */
  },
}
```

Конфигурация сканирования содержит поля:

* `path`: [XPath]-выражение, описываюшее путь до элемента;
* `then`: опциональное правило, описывающее вложенные `targets`,
  вычисляющие относительно текущего элемента.
* `extract`: опциональное поле, описывающее то,
  в каком формате достаётся значение по данному пути.
  В настоящее время поддерживается тип `text`,
  не содержащий никаких дополнительных параметров.

Пример:

```json5
{
  targets: {
    // Корневая цель.
    "Product": {
      // Путь до группы элементов
      path: "//html/body/div[1]/div/div[5]",
      // Дочерние цели.
      then: {
        get: {
          // Элемент с названием, содержащимся в заголовке 3-го уровня.
          "Name": {
            path: "/h3/text()",
            then: {
              extract: {
                Text: {},
              },
            },
          },
          // Элемент с ценой, содержащейся в спане.
          "Price": {
            path: "/span/text()",
            then: {
              extract: {
                Text: {},
              }
            }
          }
        }
      }
    }
  }
}
```

##### `continuation`

Правило, по которому определяется следующая сканируемая станица.
Содержит единственное поле `ref` с [XPath]-выражением, описывающим путь до атрибута,
в котором указан адрес следующей страницы.

> [!TIP]
> Типичный пример -- путь до атрибута `href` тега `<a>`,
> соответствующего кнопке перехода на следующую страницу.

описывающим путь до элемента `<a>`,
Пример:

```json5
{
  // Адрес следующей страницы берётся из ссылки по указанному пути.
  continuation: {
    "ref": "//html/body/div[1]/a/@href"
  }
}
```

[Rust]: https://play.rust-lang.org/
[XPath]: https://www.w3.org/TR/xpath-31/