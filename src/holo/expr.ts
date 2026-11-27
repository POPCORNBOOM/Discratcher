/**
 * Safe mathematical expression parser and compiler for animation matrix cells.
 * No `eval` or `new Function`.
 *
 * Supported:
 * - Numeric literals (e.g. `1`, `0.5`, `.5`, `1e-3`)
 * - Variable `t`, constant `pi`
 * - Binary operators: `+`, `-`, `*`, `/`, `^` (right-associative)
 * - Unary `+`, `-`
 * - Parentheses `(...)`
 * - Functions: `sin`, `cos`, `tan` (arguments in DEGREES), `sqrt`, `abs`, `min`, `max`
 */

export type CompiledExpr = (t: number) => number

export type CompileResult =
  | { ok: true; fn: CompiledExpr }
  | { ok: false; error: string }

type TokenKind = 'num' | 'id' | 'op' | 'lparen' | 'rparen' | 'comma' | 'eof'

interface Token {
  kind: TokenKind
  text: string
  pos: number
  value?: number
}

const DEG2RAD = Math.PI / 180

const UNARY_FUNCS: Record<string, (x: number) => number> = {
  sin: (x) => Math.sin(x * DEG2RAD),
  cos: (x) => Math.cos(x * DEG2RAD),
  tan: (x) => Math.tan(x * DEG2RAD),
  sqrt: (x) => Math.sqrt(x),
  abs: (x) => Math.abs(x),
}

const VARIADIC_FUNCS: Record<string, (args: number[]) => number> = {
  min: (args) => Math.min(...args),
  max: (args) => Math.max(...args),
}

function tokenize(src: string): { ok: true; tokens: Token[] } | { ok: false; error: string } {
  const tokens: Token[] = []
  let i = 0
  const n = src.length
  while (i < n) {
    const ch = src[i]
    if (ch === ' ' || ch === '\t' || ch === '\n' || ch === '\r') {
      i++
      continue
    }
    if ((ch >= '0' && ch <= '9') || (ch === '.' && i + 1 < n && src[i + 1] >= '0' && src[i + 1] <= '9')) {
      const start = i
      let hasDot = false
      while (i < n && ((src[i] >= '0' && src[i] <= '9') || (!hasDot && src[i] === '.'))) {
        if (src[i] === '.') hasDot = true
        i++
      }
      if (i < n && (src[i] === 'e' || src[i] === 'E')) {
        const expStart = i
        i++
        if (i < n && (src[i] === '+' || src[i] === '-')) i++
        const digitsStart = i
        while (i < n && src[i] >= '0' && src[i] <= '9') i++
        if (i === digitsStart) {
          return { ok: false, error: `Invalid exponent at position ${expStart + 1}` }
        }
      }
      const text = src.slice(start, i)
      const value = Number(text)
      if (!Number.isFinite(value)) {
        return { ok: false, error: `Invalid number "${text}"` }
      }
      tokens.push({ kind: 'num', text, pos: start, value })
      continue
    }
    if ((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || ch === '_') {
      const start = i
      i++
      while (
        i < n &&
        ((src[i] >= 'a' && src[i] <= 'z') ||
          (src[i] >= 'A' && src[i] <= 'Z') ||
          (src[i] >= '0' && src[i] <= '9') ||
          src[i] === '_')
      ) {
        i++
      }
      tokens.push({ kind: 'id', text: src.slice(start, i), pos: start })
      continue
    }
    if (ch === '+' || ch === '-' || ch === '*' || ch === '/' || ch === '^') {
      tokens.push({ kind: 'op', text: ch, pos: i })
      i++
      continue
    }
    if (ch === '(') {
      tokens.push({ kind: 'lparen', text: ch, pos: i })
      i++
      continue
    }
    if (ch === ')') {
      tokens.push({ kind: 'rparen', text: ch, pos: i })
      i++
      continue
    }
    if (ch === ',') {
      tokens.push({ kind: 'comma', text: ch, pos: i })
      i++
      continue
    }
    return { ok: false, error: `Unexpected character "${ch}" at position ${i + 1}` }
  }
  tokens.push({ kind: 'eof', text: '', pos: n })
  return { ok: true, tokens }
}

class ParseError extends Error {}

class Parser {
  private idx = 0
  private readonly tokens: Token[]
  constructor(tokens: Token[]) {
    this.tokens = tokens
  }

  private peek(): Token {
    return this.tokens[this.idx]
  }

  private next(): Token {
    return this.tokens[this.idx++]
  }

  parse(): CompiledExpr {
    if (this.peek().kind === 'eof') {
      throw new ParseError('Empty expression')
    }
    const fn = this.parseAdd()
    const rem = this.peek()
    if (rem.kind !== 'eof') {
      throw new ParseError(`Unexpected "${rem.text}" at position ${rem.pos + 1}`)
    }
    return fn
  }

  private parseAdd(): CompiledExpr {
    let left = this.parseMul()
    while (this.peek().kind === 'op' && (this.peek().text === '+' || this.peek().text === '-')) {
      const op = this.next().text
      const right = this.parseMul()
      const l = left
      left = op === '+' ? (t) => l(t) + right(t) : (t) => l(t) - right(t)
    }
    return left
  }

  private parseMul(): CompiledExpr {
    let left = this.parseUnary()
    while (this.peek().kind === 'op' && (this.peek().text === '*' || this.peek().text === '/')) {
      const op = this.next().text
      const right = this.parseUnary()
      const l = left
      left = op === '*' ? (t) => l(t) * right(t) : (t) => l(t) / right(t)
    }
    return left
  }

  private parseUnary(): CompiledExpr {
    if (this.peek().kind === 'op' && (this.peek().text === '+' || this.peek().text === '-')) {
      const op = this.next().text
      const sub = this.parseUnary()
      return op === '-' ? (t) => -sub(t) : sub
    }
    return this.parsePow()
  }

  private parsePow(): CompiledExpr {
    const base = this.parsePrimary()
    if (this.peek().kind === 'op' && this.peek().text === '^') {
      this.next()
      const exp = this.parseUnary()
      return (t) => Math.pow(base(t), exp(t))
    }
    return base
  }

  private parsePrimary(): CompiledExpr {
    const tok = this.peek()
    if (tok.kind === 'num') {
      this.next()
      const v = tok.value!
      return () => v
    }
    if (tok.kind === 'lparen') {
      this.next()
      const inner = this.parseAdd()
      if (this.peek().kind !== 'rparen') {
        throw new ParseError(`Expected ")" at position ${this.peek().pos + 1}`)
      }
      this.next()
      return inner
    }
    if (tok.kind === 'id') {
      this.next()
      const name = tok.text.toLowerCase()
      if (this.peek().kind === 'lparen') {
        this.next()
        const args: CompiledExpr[] = []
        if (this.peek().kind !== 'rparen') {
          args.push(this.parseAdd())
          while (this.peek().kind === 'comma') {
            this.next()
            args.push(this.parseAdd())
          }
        }
        if (this.peek().kind !== 'rparen') {
          throw new ParseError(`Expected ")" after arguments to ${tok.text}`)
        }
        this.next()

        const unary = UNARY_FUNCS[name]
        if (unary) {
          if (args.length !== 1) {
            throw new ParseError(`${tok.text}() expects 1 argument, got ${args.length}`)
          }
          const arg0 = args[0]
          return (t) => unary(arg0(t))
        }
        const variadic = VARIADIC_FUNCS[name]
        if (variadic) {
          if (args.length < 2) {
            throw new ParseError(`${tok.text}() expects at least 2 arguments, got ${args.length}`)
          }
          return (t) => variadic(args.map((a) => a(t)))
        }
        throw new ParseError(`Unknown function "${tok.text}"`)
      }
      if (name === 't') return (t) => t
      if (name === 'pi') return () => Math.PI
      throw new ParseError(`Unknown identifier "${tok.text}"`)
    }
    throw new ParseError(
      tok.kind === 'eof'
        ? 'Unexpected end of expression'
        : `Unexpected "${tok.text}" at position ${tok.pos + 1}`,
    )
  }
}

export function compileExpr(source: string): CompileResult {
  const lexed = tokenize(source)
  if (!lexed.ok) return lexed
  try {
    const fn = new Parser(lexed.tokens).parse()
    return { ok: true, fn }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}
