// Copperframe C# scripting host: the entry points the engine calls
// (copperframe_game::csharp). Part of the engine, compiled together with
// the project's scripts; do not edit the copy in .psx/csharp/.

#nullable enable

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace Copperframe.Internal
{
    /// <summary>A value crossing the engine boundary (matches NativeValue in Rust).</summary>
    [StructLayout(LayoutKind.Sequential)]
    internal unsafe struct NativeValue
    {
        public const int TagNil = 0, TagBool = 1, TagInt = 2, TagNum = 3, TagStr = 4;

        public int Tag;
        public int Len;
        public long I;
        public double F;
        public byte* S;

        public static NativeValue Bool(bool b) => new NativeValue { Tag = TagBool, I = b ? 1 : 0 };
        public static NativeValue Int(long i) => new NativeValue { Tag = TagInt, I = i };
        public static NativeValue Num(double f) => new NativeValue { Tag = TagNum, F = f };
        public static NativeValue Str(byte* s, int len) => new NativeValue { Tag = TagStr, S = s, Len = len };

        public double AsNumber() => Tag switch
        {
            TagInt => I,
            TagNum => F,
            _ => 0,
        };

        public string? AsString() => Tag == TagStr ? Encoding.UTF8.GetString(S, Len) : null;
    }

    /// <summary>A callback to run (matches InvokeArgs in Rust).</summary>
    [StructLayout(LayoutKind.Sequential)]
    internal unsafe struct InvokeArgs
    {
        public int Callback;
        public int Flag;
        public IntPtr State;
        public long Me;
        public long Other;
        public byte* Text;
        public int TextLen;
        public int ErrorLen;
        public IntPtr Error;
    }

    internal sealed class Kind
    {
        public required Type Type;
        public required string Name;
        public required List<(FieldInfo Field, string Name, string FieldKind)> Fields;
    }

    internal static unsafe class Host
    {
        /// <summary>Version of the entry points; the engine checks it.</summary>
        public const int AbiVersion = 1;

        internal static delegate* unmanaged<IntPtr, byte*, int, NativeValue*, int, NativeValue*, int, int> CtxCall;

        // The engine may run games on several threads at once (tests do), and
        // they share this class: the instance table is locked, the Ctx and
        // its buffers are per thread.
        private static readonly object Lock = new();
        private static readonly List<Kind> Kinds = new();
        private static readonly Dictionary<long, Behaviour> Instances = new();
        private static long nextHandle = 1;
        private static string? described;
        [ThreadStatic] private static Ctx? ctx;
        private static string scriptsDir = "";

        private static Ctx ThreadCtx()
        {
            if (ctx == null)
            {
                var args = (NativeValue*)NativeMemory.AllocZeroed((nuint)(Ctx.MaxArgs * sizeof(NativeValue)));
                var outs = (NativeValue*)NativeMemory.AllocZeroed((nuint)(Ctx.MaxOuts * sizeof(NativeValue)));
                ctx = new Ctx(args, outs);
            }
            return ctx;
        }

        [UnmanagedCallersOnly]
        public static int Init(IntPtr ctxCall, byte* projectRoot, int len)
        {
            CtxCall = (delegate* unmanaged<IntPtr, byte*, int, NativeValue*, int, NativeValue*, int, int>)ctxCall;
            scriptsDir = Path.GetFullPath(Path.Combine(Encoding.UTF8.GetString(projectRoot, len), "scripts"));
            return AbiVersion;
        }

        [UnmanagedCallersOnly]
        public static void Free(IntPtr p) => NativeMemory.Free((void*)p);

        private static IntPtr Utf8(string s, int* len)
        {
            var bytes = Encoding.UTF8.GetBytes(s);
            var p = (byte*)NativeMemory.Alloc((nuint)Math.Max(bytes.Length, 1));
            bytes.CopyTo(new Span<byte>(p, bytes.Length));
            *len = bytes.Length;
            return (IntPtr)p;
        }

        /// <summary>snake_case of a C# name: DoorOpener is door_opener.</summary>
        internal static string Snake(string name)
        {
            var sb = new StringBuilder();
            for (int i = 0; i < name.Length; i++)
            {
                char c = name[i];
                if (char.IsUpper(c))
                {
                    bool boundary = i > 0 && (char.IsLower(name[i - 1]) || char.IsDigit(name[i - 1])
                        || (i + 1 < name.Length && char.IsLower(name[i + 1]) && char.IsUpper(name[i - 1])));
                    if (boundary) sb.Append('_');
                    sb.Append(char.ToLowerInvariant(c));
                }
                else sb.Append(c);
            }
            return sb.ToString();
        }

        private static string? FieldKind(Type t) =>
            t == typeof(float) || t == typeof(double) || t == typeof(int) ? "number"
            : t == typeof(bool) ? "bool"
            : t == typeof(string) ? "text"
            : t == typeof(EntityRef) ? "entity"
            : null;

        private static object? DefaultJson(object? v) => v switch
        {
            float f => (double)f,
            double d => d,
            int i => (double)i,
            bool b => b,
            string s => s,
            EntityRef => "",
            _ => null,
        };

        /// <summary>
        /// Finds the behaviours and returns them as JSON:
        /// {"kinds":[{kind, class, description, folder, fields:[{name, kind, default, help}]}],
        ///  "problems":[{class, message}]}.
        /// </summary>
        [UnmanagedCallersOnly]
        public static IntPtr Describe(int* len)
        {
            lock (Lock)
            {
                described ??= DescribeKinds();
                return Utf8(described, len);
            }
        }

        private static string DescribeKinds()
        {
            var kinds = new List<object>();
            var problems = new List<object>();
            Kinds.Clear();
            var types = typeof(Host).Assembly.GetTypes()
                .Where(t => t.IsClass && !t.IsAbstract && t.IsPublic && typeof(Behaviour).IsAssignableFrom(t))
                .OrderBy(t => t.FullName, StringComparer.Ordinal);
            foreach (var t in types)
            {
                try
                {
                    var attr = t.GetCustomAttribute<BehaviourAttribute>();
                    var name = attr?.Name ?? Snake(t.Name);
                    if (t.GetConstructor(Type.EmptyTypes) == null)
                    {
                        problems.Add(new { @class = t.FullName, message = $"{t.Name} needs a public constructor without parameters" });
                        continue;
                    }
                    var sample = (Behaviour)Activator.CreateInstance(t)!;
                    var fields = new List<(FieldInfo, string, string)>();
                    var fieldJson = new List<object>();
                    foreach (var f in t.GetFields(BindingFlags.Public | BindingFlags.Instance))
                    {
                        if (f.GetCustomAttribute<HideAttribute>() != null || f.IsInitOnly) continue;
                        var kind = FieldKind(f.FieldType);
                        if (kind == null) continue;
                        var fname = Snake(f.Name);
                        fields.Add((f, fname, kind));
                        fieldJson.Add(new
                        {
                            name = fname,
                            kind,
                            @default = DefaultJson(f.GetValue(sample)),
                            help = f.GetCustomAttribute<HelpAttribute>()?.Text ?? "",
                        });
                    }
                    Kinds.Add(new Kind { Type = t, Name = name, Fields = fields });
                    kinds.Add(new
                    {
                        kind = "cs." + name,
                        @class = t.FullName,
                        description = attr?.Description ?? "",
                        folder = attr?.Folder ?? "",
                        fields = fieldJson,
                    });
                }
                catch (Exception e)
                {
                    problems.Add(new { @class = t.FullName, message = $"{t.Name} could not be created: {Unwrap(e).Message}" });
                }
            }
            return JsonSerializer.Serialize(new { kinds, problems });
        }

        private static Exception Unwrap(Exception e) =>
            e is TargetInvocationException { InnerException: { } inner } ? inner : e;

        /// <summary>A new behaviour of Kinds[kind] with the scene's values (JSON object).</summary>
        [UnmanagedCallersOnly]
        public static long Create(int kind, byte* paramsJson, int paramsLen, IntPtr* error, int* errorLen)
        {
            try
            {
                Kind k;
                lock (Lock) k = Kinds[kind];
                var b = (Behaviour)Activator.CreateInstance(k.Type)!;
                using var doc = JsonDocument.Parse(new ReadOnlyMemory<byte>(new ReadOnlySpan<byte>(paramsJson, paramsLen).ToArray()));
                foreach (var (field, name, fieldKind) in k.Fields)
                {
                    if (!doc.RootElement.TryGetProperty(name, out var v)) continue;
                    object? value = fieldKind switch
                    {
                        "number" when field.FieldType == typeof(float) => (float)v.GetDouble(),
                        "number" when field.FieldType == typeof(int) => (int)Math.Round(v.GetDouble()),
                        "number" => v.GetDouble(),
                        "bool" => v.GetBoolean(),
                        "text" => v.GetString() ?? "",
                        "entity" => new EntityRef(v.GetString() ?? ""),
                        _ => null,
                    };
                    field.SetValue(b, value);
                }
                lock (Lock)
                {
                    long handle = nextHandle++;
                    Instances[handle] = b;
                    return handle;
                }
            }
            catch (Exception e)
            {
                *error = Utf8(Unwrap(e).Message, errorLen);
                return 0;
            }
        }

        [UnmanagedCallersOnly]
        public static void Release(long handle)
        {
            lock (Lock) Instances.Remove(handle);
        }

        private static Behaviour? Find(long handle)
        {
            lock (Lock) return Instances.TryGetValue(handle, out var b) ? b : null;
        }

        /// <summary>The behaviour's Prompt, or null.</summary>
        [UnmanagedCallersOnly]
        public static IntPtr Prompt(long handle, int* len)
        {
            var prompt = Find(handle)?.Prompt;
            return prompt == null ? IntPtr.Zero : Utf8(prompt, len);
        }

        /// <summary>
        /// Runs callback (0 start, 1 fixed_update, 2 on_trigger, 3 on_collision,
        /// 4 on_interact, 5 on_message, 6 on_destroy). Returns 0, or 1 with an
        /// error as JSON {message, file, line}.
        /// </summary>
        [UnmanagedCallersOnly]
        public static int Invoke(long handle, InvokeArgs* a)
        {
            var b = Find(handle);
            if (b == null) return 0;
            b.Entity = new Entity(a->Me);
            var c = ThreadCtx();
            c.State = a->State;
            try
            {
                var o = new Entity(a->Other);
                switch (a->Callback)
                {
                    case 0: b.Start(c); break;
                    case 1: b.FixedUpdate(c); break;
                    case 2: b.OnTrigger(c, o, a->Flag != 0); break;
                    case 3: b.OnCollision(c, o, a->Flag != 0); break;
                    case 4: b.OnInteract(c, o); break;
                    case 5: b.OnMessage(c, Encoding.UTF8.GetString(a->Text, a->TextLen), a->Other == 0 ? null : o); break;
                    case 6: b.OnDestroy(c); break;
                }
                return 0;
            }
            catch (Exception e)
            {
                int len;
                a->Error = Utf8(Report(Unwrap(e)), &len);
                a->ErrorLen = len;
                return 1;
            }
            finally
            {
                c.State = IntPtr.Zero;
            }
        }

        /// <summary>The exception's message and the script line it came from.</summary>
        private static string Report(Exception e)
        {
            string? file = null;
            int line = 0;
            foreach (var frame in new StackTrace(e, true).GetFrames())
            {
                var f = frame.GetFileName();
                if (f != null && Path.GetFullPath(f).StartsWith(scriptsDir, StringComparison.OrdinalIgnoreCase))
                {
                    file = Path.GetRelativePath(Path.GetDirectoryName(scriptsDir)!, Path.GetFullPath(f)).Replace('\\', '/');
                    line = frame.GetFileLineNumber();
                    break;
                }
            }
            string message = e is CtxException ? e.Message : $"{e.GetType().Name}: {e.Message}";
            return JsonSerializer.Serialize(new { message, file, line });
        }
    }
}
