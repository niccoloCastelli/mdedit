# Feature tour

Lead paragraph with *emphasis*, **strong**, ~~strike~~ and `code`.

## Text that needs escaping

Typst specials: #hash $dollar *star* _under_ [brackets] @at ~tilde \backslash
// not a comment, -- not a dash, `` a`b ``, 3 < 4 > 2.
- not a list item
12. not an enumeration
= not a heading

Link to [a heading](#lists-and-tasks), a [missing anchor](#nope) and
[the web](https://example.com/a_b?c=d "Title").[^note]

## Lists and tasks

- one
- two
  - nested **bold**
- three

3. three
4. four

- [ ] open task
- [x] done task
- plain item in a task list

1. Loose item.

   Second paragraph of the loose item.

2. Another loose item.

## Blocks

> A plain quote
> over two lines.

> [!NOTE]
> A GitHub-style note.

> **Remember**
> A callout with a bold title.

```python
def greet(name):
    return f"hi {name}"  # comment
```

```
plain text block
```

| Left | Centre | Right |
|:-----|:------:|------:|
| a    | b      | c     |
| `x`  | **y**  |       |

![Missing image](missing.png)

Inline ![icon](diagram.png) image.

---

Last paragraph[^note] with a hard\
break.

[^note]: A footnote with *style*.
