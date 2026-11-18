import { describe, expect, it } from "vitest";
import { pathKey, pathToUri, uriKey, uriToPath } from "./uri";

describe("pathToUri", () => {
  it("path POSIX", () => {
    expect(pathToUri("/home/user/project/src/main.rs")).toBe("file:///home/user/project/src/main.rs");
    expect(pathToUri("/")).toBe("file:///");
  });

  it("huruf drive Windows, dengan pemisah mana pun", () => {
    expect(pathToUri("C:\\x\\y")).toBe("file:///C:/x/y");
    expect(pathToUri("C:/x/y")).toBe("file:///C:/x/y");
    expect(pathToUri("d:\\Projects\\app\\main.go")).toBe("file:///d:/Projects/app/main.go");
    expect(pathToUri("C:\\")).toBe("file:///C:/");
  });

  it("path UNC", () => {
    expect(pathToUri("\\\\server\\share\\dir\\a b.ts")).toBe("file://server/share/dir/a%20b.ts");
  });

  it("meng-escape spasi, karakter khusus URI, dan non-ASCII", () => {
    expect(pathToUri("/home/a b/c.rs")).toBe("file:///home/a%20b/c.rs");
    expect(pathToUri("/tmp/what?#100%.txt")).toBe("file:///tmp/what%3F%23100%25.txt");
    expect(pathToUri("/home/é/日本語.py")).toBe("file:///home/%C3%A9/%E6%97%A5%E6%9C%AC%E8%AA%9E.py");
    expect(pathToUri("C:\\Users\\Dé v\\a#b.ts")).toBe("file:///C:/Users/D%C3%A9%20v/a%23b.ts");
  });

  it("garis miring terbalik di POSIX adalah bagian nama berkas", () => {
    expect(pathToUri("/tmp/a\\b.txt")).toBe("file:///tmp/a%5Cb.txt");
    expect(uriToPath("file:///tmp/a%5Cb.txt")).toBe("/tmp/a\\b.txt");
  });
});

describe("uriToPath", () => {
  it("bolak-balik untuk path POSIX", () => {
    for (const path of ["/home/user/main.rs", "/home/a b/c d.rs", "/tmp/what?#100%.txt", "/home/é/日本語.py", "/srv/@types/node/index.d.ts", "/x/[id]/(group)/+page.ts", "/"]) {
      expect(uriToPath(pathToUri(path)), path).toBe(path);
    }
  });

  it("bolak-balik untuk path Windows", () => {
    for (const path of ["C:\\x\\y", "C:\\Users\\Dé v\\a#b?.ts", "D:\\a b\\c.go", "\\\\server\\share\\dir\\a b.ts"]) {
      expect(uriToPath(pathToUri(path)), path).toBe(path);
    }
    // Pemisah dan huruf drive dinormalkan.
    expect(uriToPath(pathToUri("c:/x/y"))).toBe("C:\\x\\y");
  });

  it("menerima bentuk yang dikirim server: titik dua ter-escape, huruf drive kecil, escape berbeda", () => {
    expect(uriToPath("file:///c%3A/Users/me/a.ts")).toBe("C:\\Users\\me\\a.ts");
    expect(uriToPath("file:///C:/Users/me/a.ts")).toBe("C:\\Users\\me\\a.ts");
    expect(uriToPath("file:///srv/%40types/node/index.d.ts")).toBe("/srv/@types/node/index.d.ts");
    expect(uriToPath("file:///srv/@types/node/index.d.ts")).toBe("/srv/@types/node/index.d.ts");
    expect(uriToPath("file://localhost/home/user/a.rs")).toBe("/home/user/a.rs");
  });

  it("membuang query dan fragment, dan tidak gagal pada escape yang rusak", () => {
    expect(uriToPath("file:///home/a.rs#L10")).toBe("/home/a.rs");
    expect(uriToPath("file:///home/a.rs?x=1")).toBe("/home/a.rs");
    expect(uriToPath("file:///home/100%zz.txt")).toBe("/home/100%zz.txt");
  });

  it("URI non-file dikembalikan apa adanya", () => {
    expect(uriToPath("untitled:Untitled-1")).toBe("untitled:Untitled-1");
    expect(uriToPath("jdt://contents/rt.jar/java.lang/String.class")).toBe("jdt://contents/rt.jar/java.lang/String.class");
  });
});

describe("kunci pembanding", () => {
  it("URI yang sama dengan escape berbeda menghasilkan kunci yang sama", () => {
    expect(uriKey("file:///srv/%40types/a.ts")).toBe(uriKey("file:///srv/@types/a.ts"));
    expect(uriKey("file:///c%3A/Users/Me/a.ts")).toBe(uriKey(pathToUri("C:\\Users\\me\\A.ts")));
  });

  it("path POSIX peka huruf besar/kecil", () => {
    expect(pathKey("/home/A.rs")).not.toBe(pathKey("/home/a.rs"));
    expect(pathKey("C:/X/y")).toBe(pathKey("c:\\x\\Y"));
  });
});
