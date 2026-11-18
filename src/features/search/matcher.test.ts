// Harapan di sini mengikuti pengujian backend (`crates/oncode-workspace/src/search.rs`): pencocok JS
// harus menemukan kecocokan yang sama dan menghasilkan teks pengganti yang sama.
import { describe, expect, it } from "vitest";
import { applyEdits, compileMatcher, computeEdits, expandReplacement, translateRegex, type MatchOptions } from "./matcher";

const literal = (pattern: string, extra: Partial<MatchOptions> = {}): MatchOptions => ({ pattern, isRegex: false, caseSensitive: false, wholeWord: false, ...extra });
const regex = (pattern: string, extra: Partial<MatchOptions> = {}): MatchOptions => ({ pattern, isRegex: true, caseSensitive: true, wholeWord: false, ...extra });

function matcherOf(options: MatchOptions) {
  const compiled = compileMatcher(options);
  if (!compiled.ok) throw new Error(`tidak bisa disusun: ${compiled.reason}`);
  return compiled.matcher;
}

/** [kolom, panjang] tiap kecocokan pada satu baris. */
const spans = (options: MatchOptions, line: string) => matcherOf(options).find(line).map((h) => [h.start, h.end - h.start]);
const columns = (options: MatchOptions, line: string) => spans(options, line).map(([start]) => start);

/** Isi berkas setelah semua kecocokan diganti, seperti `replace_in_files` di backend. */
function replaced(options: MatchOptions, replacement: string, text: string): string {
  const lines = text.split("\n");
  return applyEdits(lines, computeEdits(matcherOf(options), lines, replacement, options.isRegex)).join("\n");
}

const reasonFor = (pattern: string) => {
  const compiled = compileMatcher(regex(pattern));
  return compiled.ok ? null : compiled.reason;
};

describe("literal search", () => {
  it("does not interpret regex syntax", () => {
    expect(spans(literal("a.b("), "let x = a.b(c);")).toEqual([[8, 4]]);
    expect(spans(literal("a.b("), "let y = aXb(c);")).toEqual([]);
    expect(columns(literal("[x]{2}$^|\\d/"), "see [x]{2}$^|\\d/ here")).toEqual([4]);
  });

  it("is case-insensitive unless asked otherwise", () => {
    for (const line of ["Hello", "hello", "HELLO"]) expect(columns(literal("hello"), line)).toEqual([0]);
    expect(columns(literal("hello", { caseSensitive: true }), "Hello")).toEqual([]);
    expect(columns(literal("hello", { caseSensitive: true }), "hello")).toEqual([0]);
    // Pelipatan huruf Unicode, bukan hanya ASCII.
    expect(columns(literal("ÉCOLE"), "une école")).toEqual([4]);
  });

  it("finds several non-overlapping matches on one line", () => {
    expect(columns(literal("aa"), "aaaaa aa")).toEqual([0, 2, 6]);
  });
});

describe("whole word", () => {
  it("matches the backend on its own fixture", () => {
    const line = "cat concat cat_x (cat) cats";
    expect(columns(literal("cat"), line)).toHaveLength(5);
    expect(columns(literal("cat", { wholeWord: true }), line)).toEqual([0, 18]);
    expect(columns(regex("ca.", { wholeWord: true }), line)).toEqual([0, 18]);
  });

  it("uses Unicode word characters, not ASCII ones", () => {
    // `\b` milik JS menganggap "é" bukan karakter kata dan akan salah mencocokkan "écat" dan "caté".
    expect(columns(literal("cat", { wholeWord: true }), "écat caté cat")).toEqual([10]);
    expect(columns(literal("cat", { wholeWord: true }), "猫cat cat٣ cat")).toEqual([10]);
    // Karakter astral (dua unit UTF-16) di depan kecocokan.
    expect(columns(literal("cat", { wholeWord: true }), "𝒳cat 😀cat")).toEqual([8]);
  });

  it("only constrains the two ends of the match", () => {
    expect(spans(literal("a.b", { wholeWord: true }), "x a.b y a.bc")).toEqual([[2, 3]]);
    // Batas "separuh" hanya melihat tetangga: tidak boleh ada karakter kata tepat di luar kecocokan,
    // apa pun karakter pertama dan terakhir kecocokan itu sendiri.
    expect(spans(literal("(cat)", { wholeWord: true }), "x (cat) y")).toEqual([[2, 5]]);
    expect(spans(literal("(cat)", { wholeWord: true }), "a(cat) (cat)b")).toEqual([]);
  });

  it("backtracks into the pattern to end on a word boundary", () => {
    expect(spans(regex("\\w+?", { wholeWord: true }), "abc de")).toEqual([[0, 3], [4, 2]]);
  });
});

describe("regex search", () => {
  it("anchors per line and never reports empty matches", () => {
    expect(columns(regex("^foo"), "foo bar")).toEqual([0]);
    expect(columns(regex("^foo"), "bar foo")).toEqual([]);
    expect(columns(regex("foo$"), "bar foo")).toEqual([4]);
    expect(spans(regex("foo\\s*$"), "bar foo  ")).toEqual([[4, 5]]);
    expect(spans(regex("a*"), "bab")).toEqual([[1, 1]]);
    expect(spans(regex("^"), "ccc")).toEqual([]);
  });

  it("honours case sensitivity", () => {
    expect(columns(regex("h[a-z]llo"), "Hello hello")).toEqual([6]);
    expect(columns(regex("h[a-z]llo", { caseSensitive: false }), "Hello hello HELLO")).toEqual([0, 6, 12]);
  });

  it("gives \\w, \\d, \\s and \\b their Unicode meaning, as the regex crate does", () => {
    expect(spans(regex("\\w+"), "héllo wörld_1")).toEqual([[0, 5], [6, 7]]);
    expect(spans(regex("\\d+"), "a ٣٤ b 12")).toEqual([[2, 2], [7, 2]]);
    expect(spans(regex("\\bé\\w+"), "été école")).toEqual([[0, 3], [4, 5]]);
    expect(spans(regex("\\W+"), "é—é")).toEqual([[1, 1]]);
    expect(spans(regex("[\\w-]+"), "naïve-ish !")).toEqual([[0, 9]]);
    expect(spans(regex("a\\sb"), "a\u00a0b a\u3000b")).toEqual([[0, 3], [4, 3]]);
    expect(spans(regex("\\S+"), "ab  cd")).toEqual([[0, 2], [4, 2]]);
    expect(spans(regex("\\Bb"), "ab b")).toEqual([[1, 1]]);
  });

  it("reports UTF-16 offsets and treats an astral character as one character", () => {
    const line = "héllo 😀 wörld 😀😀!";
    expect(spans(literal("wörld"), line)).toEqual([[9, 5]]);
    expect(spans(regex("😀."), line)).toEqual([[6, 3], [15, 4]]);
    expect(spans(regex("."), "😀a")).toEqual([[0, 2], [2, 1]]);
    expect(spans(regex("[^a]"), "a😀")).toEqual([[1, 2]]);
    // Kelas dan `\\w` tepat sebelum akhir baris dengan karakter astral di ujung.
    expect(spans(regex("\\w$"), "x𝒳")).toEqual([[1, 2]]);
    expect(spans(regex("[^a]$"), "a😀")).toEqual([[1, 2]]);
    expect(spans(regex("cat", { wholeWord: true }), "cat𝒳 cat")).toEqual([[6, 3]]);
    // Kecocokan kosong di depan karakter astral tidak membelah pasangan surrogate.
    expect(spans(regex("x*"), "😀x")).toEqual([[2, 1]]);
  });

  it("translates Rust-only syntax", () => {
    expect(translateRegex("(?P<n>a)")).toBe("(?<n>a)");
    expect(spans(regex("(?P<year>\\d{4})-(?<month>\\d\\d)"), "on 2026-11-17")).toEqual([[3, 7]]);
    expect(spans(regex("\\x41\\x{1F600}\\u{42}\\U0001F600"), "A😀B😀")).toEqual([[0, 6]]);
    expect(spans(regex("\\pL+"), "ab1γδ")).toEqual([[0, 2], [3, 2]]);
    expect(spans(regex("\\p{Greek}+"), "abγδ")).toEqual([[2, 2]]);
    expect(spans(regex("\\P{L}+"), "a12b")).toEqual([[1, 2]]);
    expect(spans(regex("\\p{sc:Latin}+|\\p{gc=Nd}+"), "ab٣γ")).toEqual([[0, 2], [2, 1]]);
    expect(spans(regex("[]a]+"), "x]a]y")).toEqual([[1, 3]]);
    expect(spans(regex("a]}"), "a]} a]")).toEqual([[0, 3]]);
    expect(spans(regex("\\-\\#\\ \\&"), "x-# &")).toEqual([[1, 4]]);
    expect(spans(regex("[\\-\\]\\\\^]+"), "a-]\\^b")).toEqual([[1, 4]]);
    expect(spans(regex("a{2,3}?"), "aaaa")).toEqual([[0, 2], [2, 2]]);
    expect(spans(regex("x/y"), "x/y")).toEqual([[0, 3]]);
  });

  it("refuses constructs it cannot reproduce exactly instead of guessing", () => {
    expect(reasonFor("(?i)abc")).toMatch(/inline flags/i);
    expect(reasonFor("(?s:a.b)")).toMatch(/inline flags/i);
    expect(reasonFor("[[:alpha:]]+")).toMatch(/POSIX/);
    expect(reasonFor("[a-z&&[^b]]")).toMatch(/character class/i);
    expect(reasonFor("[a-z--b]")).toMatch(/set operations/);
    expect(reasonFor("[\\W]")).toMatch(/\\W/);
    expect(reasonFor("\\<word\\>")).toMatch(/escape/);
    expect(reasonFor("\\b{start}x")).toMatch(/word boundary/);
    expect(reasonFor("(a)\\1")).toMatch(/escape \\1/);
    // Backend sendiri tidak konsisten soal `\A`/`\z` antara pencarian dan penggantian.
    expect(reasonFor("\\w\\z")).toMatch(/anchor \\z/);
    expect(reasonFor("\\Afoo")).toMatch(/anchor \\A/);
    expect(reasonFor("\\p{NoSuchClass}")).toMatch(/Unicode class/);
    expect(reasonFor("\\p{^L}")).toMatch(/Unicode class/);
    expect(reasonFor("a{,3}")).toMatch(/\{/);
    expect(reasonFor("a**")).not.toBeNull();
    expect(reasonFor("(?P<a.b>x)")).not.toBeNull();
    expect(compileMatcher(literal(""))).toMatchObject({ ok: false });
  });
});

describe("replacement text", () => {
  const hit = { groups: ["whole", "one", undefined, "three"], named: { val: "V", none: undefined } };

  it("expands numbered, named and braced references", () => {
    expect(expandReplacement("$1-$3-$0", hit)).toBe("one-three-whole");
    expect(expandReplacement("${1}x ${val}y $val", hit)).toBe("onex Vy V");
  });

  it("turns $$ into a dollar and keeps a dollar that starts no reference", () => {
    expect(expandReplacement("$$1 costs $ 5 and $", hit)).toBe("$1 costs $ 5 and $");
    expect(expandReplacement("${1", hit)).toBe("${1");
    expect(expandReplacement("${a-b}", hit)).toBe("${a-b}");
    expect(expandReplacement("$-", hit)).toBe("$-");
  });

  it("takes the longest name, so $1a is a group called 1a", () => {
    expect(expandReplacement("$1a", hit)).toBe("");
    expect(expandReplacement("${1}a", hit)).toBe("onea");
    expect(expandReplacement("$val_x", hit)).toBe("");
  });

  it("uses an empty string for groups that are missing or did not participate", () => {
    expect(expandReplacement("[$2][$9][$nope][$none][$constructor]", hit)).toBe("[][][][][]");
  });
});

describe("replacing", () => {
  it("replaces literally, without interpreting $1, and leaves other lines alone", () => {
    const text = "foo(1) and Foo(2)\nnothing\nfoo(3)";
    expect(replaced(literal("foo(", { caseSensitive: true }), "$1bar[", text)).toBe("$1bar[1) and Foo(2)\nnothing\n$1bar[3)");
  });

  it("follows case and whole-word options", () => {
    expect(replaced(literal("cat", { wholeWord: true }), "dog", "Cat cats CAT")).toBe("dog cats dog");
  });

  it("expands capture references in regex mode", () => {
    const text = "let alpha = 1;\nlet beta = 22;\nconst gamma = 3;";
    expect(replaced(regex("let (\\w+) = (?P<val>\\d+);"), "const ${1}: u32 = $val; // $$", text)).toBe("const alpha: u32 = 1; // $\nconst beta: u32 = 22; // $\nconst gamma = 3;");
  });

  it("applies line anchors per line and leaves empty lines untouched", () => {
    expect(replaced(regex("\\s+$"), "", "a  \nb\n\nc ")).toBe("a\nb\n\nc");
    expect(replaced(regex("^foo"), "bar", "foo foo\nfoo")).toBe("bar foo\nbar");
  });

  it("never matches across lines", () => {
    expect(replaced(regex("bar\\s+bar"), "X", "foo bar\nbar foo")).toBe("foo bar\nbar foo");
  });

  it("keeps offsets right when several replacements on a line change its length", () => {
    expect(replaced(literal("a"), "bbb", "a-a-a 😀 a")).toBe("bbb-bbb-bbb 😀 bbb");
    expect(replaced(regex("(\\w)(\\w)"), "$2$1", "abcd 😀 éf")).toBe("badc 😀 fé");
  });

  it("reports edits as 1-based lines with UTF-16 ranges and can leave chosen matches out", () => {
    const lines = ["x 😀 foo foo", "", "foo"];
    const matcher = matcherOf(literal("foo"));
    expect(computeEdits(matcher, lines, "bar", false)).toEqual([
      { line: 1, from: 5, to: 8, insert: "bar" },
      { line: 1, from: 9, to: 12, insert: "bar" },
      { line: 3, from: 0, to: 3, insert: "bar" },
    ]);
    const kept = computeEdits(matcher, lines, "bar", false, (line, column, length) => line === 1 && column === 5 && length === 3);
    expect(applyEdits(lines, kept)).toEqual(["x 😀 foo bar", "", "bar"]);
  });
});
