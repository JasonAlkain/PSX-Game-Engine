// Copperframe C# scripting API. This file is part of the engine: it is
// copied into .psx/csharp/ and compiled together with the project's
// scripts/**/*.cs, so do not edit the copy there.
//
// A behaviour is a public class deriving from Behaviour. Its kind is
// "cs." followed by the class name in snake_case (DoorOpener is
// cs.door_opener). Public fields of type float, double, int, bool, string
// and EntityRef are its fields in the inspector; their initial values are
// the defaults.

#nullable enable

using System;
using System.Numerics;

namespace Copperframe
{
    /// <summary>An entity of the running scene. Valid while it exists.</summary>
    public readonly struct Entity : IEquatable<Entity>
    {
        internal readonly long Handle;

        internal Entity(long handle) => Handle = handle;

        public bool Equals(Entity other) => Handle == other.Handle;
        public override bool Equals(object? obj) => obj is Entity e && Equals(e);
        public override int GetHashCode() => Handle.GetHashCode();
        public static bool operator ==(Entity a, Entity b) => a.Handle == b.Handle;
        public static bool operator !=(Entity a, Entity b) => a.Handle != b.Handle;
        public override string ToString() => $"Entity({Handle})";
    }

    /// <summary>
    /// A field that names another entity of the same scene (its ID). Use
    /// <c>ctx.Find(field)</c> to get the entity.
    /// </summary>
    public readonly struct EntityRef
    {
        public readonly string Id;

        public EntityRef(string id) => Id = id ?? "";

        public bool IsSet => !string.IsNullOrEmpty(Id);
        public override string ToString() => Id;
    }

    /// <summary>The first solid collider along a ray.</summary>
    public readonly struct RaycastHit
    {
        public readonly Entity Entity;
        public readonly float Distance;

        internal RaycastHit(Entity entity, float distance)
        {
            Entity = entity;
            Distance = distance;
        }
    }

    /// <summary>Sets a behaviour's kind name, description or Add component folder.</summary>
    [AttributeUsage(AttributeTargets.Class, Inherited = false)]
    public sealed class BehaviourAttribute : Attribute
    {
        /// <summary>The kind after "cs." (a-z, 0-9 and _). Default: the class name in snake_case.</summary>
        public string? Name { get; set; }
        public string Description { get; set; } = "";
        /// <summary>Folder under Behaviours &gt; Scripts in the Add component menu.</summary>
        public string Folder { get; set; } = "";

        public BehaviourAttribute() { }

        public BehaviourAttribute(string description) => Description = description;
    }

    /// <summary>Help text shown for a field in the inspector.</summary>
    [AttributeUsage(AttributeTargets.Field, Inherited = false)]
    public sealed class HelpAttribute : Attribute
    {
        public string Text { get; }

        public HelpAttribute(string text) => Text = text;
    }

    /// <summary>Keeps a public field out of the inspector.</summary>
    [AttributeUsage(AttributeTargets.Field, Inherited = false)]
    public sealed class HideAttribute : Attribute { }

    /// <summary>
    /// Base class of C# behaviours. One instance per entity; override the
    /// callbacks you need.
    /// </summary>
    public abstract class Behaviour
    {
        /// <summary>The entity this behaviour is on.</summary>
        public Entity Entity { get; internal set; }

        /// <summary>
        /// Set to make the entity interactive: the text is shown while the
        /// player aims at it. null stops.
        /// </summary>
        public string? Prompt { get; set; }

        /// <summary>Once, on the first tick after the entity appears.</summary>
        public virtual void Start(Ctx ctx) { }

        /// <summary>Every fixed tick (60 per second).</summary>
        public virtual void FixedUpdate(Ctx ctx) { }

        /// <summary>A sensor collider on this entity started or stopped overlapping other.</summary>
        public virtual void OnTrigger(Ctx ctx, Entity other, bool entered) { }

        /// <summary>A solid collider on this entity started or stopped touching other.</summary>
        public virtual void OnCollision(Ctx ctx, Entity other, bool started) { }

        /// <summary>The player used the interact action while aiming at this entity.</summary>
        public virtual void OnInteract(Ctx ctx, Entity by) { }

        /// <summary>Another behaviour sent this entity a message.</summary>
        public virtual void OnMessage(Ctx ctx, string message, Entity? from) { }

        /// <summary>The entity is despawned or its scene unloaded.</summary>
        public virtual void OnDestroy(Ctx ctx) { }
    }

    /// <summary>A ctx call failed (a wrong argument, or an entity that is gone).</summary>
    public sealed class CtxException : Exception
    {
        public CtxException(string message) : base(message) { }
    }

    /// <summary>
    /// The game, as seen from a callback. Valid only during the callback it
    /// is passed to. Angles are degrees: X yaw, Y pitch, Z roll.
    /// </summary>
    public sealed unsafe class Ctx
    {
        internal IntPtr State;
        private readonly Internal.NativeValue* args;
        private readonly Internal.NativeValue* outs;
        private int count;

        internal const int MaxArgs = 8;
        internal const int MaxOuts = 4;

        internal Ctx(Internal.NativeValue* args, Internal.NativeValue* outs)
        {
            this.args = args;
            this.outs = outs;
        }

        // ---- calling the engine -------------------------------------

        private void Begin()
        {
            if (State == IntPtr.Zero)
                throw new InvalidOperationException("ctx can only be used during the callback it was passed to");
            count = 0;
        }

        private void Arg(long entity) => args[count++] = Internal.NativeValue.Int(entity);
        private void Arg(Entity e) => args[count++] = Internal.NativeValue.Int(e.Handle);
        private void Arg(float x) => args[count++] = Internal.NativeValue.Num(x);
        private void Arg(bool b) => args[count++] = Internal.NativeValue.Bool(b);

        private void Arg(Vector3 v)
        {
            Arg(v.X);
            Arg(v.Y);
            Arg(v.Z);
        }

        private int Call(ReadOnlySpan<byte> name, string? s1 = null, string? s2 = null, int s1At = -1, int s2At = -1)
        {
            // Strings are passed as UTF-8 for the duration of the call.
            byte[]? b1 = s1 == null ? null : System.Text.Encoding.UTF8.GetBytes(s1);
            byte[]? b2 = s2 == null ? null : System.Text.Encoding.UTF8.GetBytes(s2);
            fixed (byte* n = name)
            fixed (byte* p1 = b1)
            fixed (byte* p2 = b2)
            {
                if (s1At >= 0) args[s1At] = Internal.NativeValue.Str(p1, b1!.Length);
                if (s2At >= 0) args[s2At] = Internal.NativeValue.Str(p2, b2!.Length);
                int r = Internal.Host.CtxCall(State, n, name.Length, args, count, outs, MaxOuts);
                if (r < 0)
                    throw new CtxException("ctx." + (outs[0].AsString() ?? "call failed"));
                return r;
            }
        }

        private int CallStr(ReadOnlySpan<byte> name, string s)
        {
            Begin();
            int at = count++;
            return Call(name, s, null, at);
        }

        private void Unit(ReadOnlySpan<byte> name) => Call(name);
        private Entity? OutEntity(int i) => outs[i].Tag == Internal.NativeValue.TagInt ? new Entity(outs[i].I) : null;
        private bool OutBool(int i) => outs[i].Tag == Internal.NativeValue.TagBool && outs[i].I != 0;
        private float OutNum(int i) => (float)outs[i].AsNumber();
        private Vector3 OutVec() => new Vector3(OutNum(0), OutNum(1), OutNum(2));

        // ---- time and identity --------------------------------------

        /// <summary>Seconds per tick (1/60).</summary>
        public float Dt() { Begin(); Call("dt"u8); return OutNum(0); }

        /// <summary>Ticks since the game started.</summary>
        public long Tick() { Begin(); Call("tick"u8); return (long)outs[0].AsNumber(); }

        /// <summary>The entity of this behaviour.</summary>
        public Entity Me() { Begin(); Call("me"u8); return OutEntity(0)!.Value; }

        /// <summary>The entity with this ID, or null.</summary>
        public Entity? Find(string id) { CallStr("find"u8, id); return OutEntity(0); }

        /// <summary>The entity an entity field names, or null.</summary>
        public Entity? Find(EntityRef field) => field.IsSet ? Find(field.Id) : null;

        /// <summary>The first entity with this name, or null.</summary>
        public Entity? FindByName(string name) { CallStr("find_by_name"u8, name); return OutEntity(0); }

        /// <summary>The entity's ID string.</summary>
        public string? IdOf(Entity e) { Begin(); Arg(e); Call("id_of"u8); return outs[0].AsString(); }

        /// <summary>The entity's name.</summary>
        public string Name(Entity e) { Begin(); Arg(e); Call("name"u8); return outs[0].AsString() ?? ""; }

        /// <summary>Whether the entity is still there.</summary>
        public bool Exists(Entity e) { Begin(); Arg(e); Call("exists"u8); return OutBool(0); }

        /// <summary>Its parent, or null.</summary>
        public Entity? Parent(Entity e) { Begin(); Arg(e); Call("parent"u8); return OutEntity(0); }

        // ---- transforms ---------------------------------------------

        /// <summary>Position relative to the parent.</summary>
        public Vector3 Position(Entity e) { Begin(); Arg(e); Call("position"u8); return OutVec(); }

        /// <summary>Moves it (relative to the parent).</summary>
        public void SetPosition(Entity e, Vector3 p) { Begin(); Arg(e); Arg(p); Unit("set_position"u8); }

        /// <summary>Moves it by an offset.</summary>
        public void Translate(Entity e, Vector3 d) { Begin(); Arg(e); Arg(d); Unit("translate"u8); }

        /// <summary>Position in the world, as of the last transform update.</summary>
        public Vector3 WorldPosition(Entity e) { Begin(); Arg(e); Call("world_position"u8); return OutVec(); }

        /// <summary>Rotation in degrees: X yaw, Y pitch, Z roll.</summary>
        public Vector3 Rotation(Entity e) { Begin(); Arg(e); Call("rotation"u8); return OutVec(); }

        /// <summary>Sets the rotation (degrees: yaw, pitch, roll).</summary>
        public void SetRotation(Entity e, float yaw, float pitch = 0, float roll = 0)
        { Begin(); Arg(e); Arg(yaw); Arg(pitch); Arg(roll); Unit("set_rotation"u8); }

        /// <summary>Turns it by these angles about its own axes.</summary>
        public void Rotate(Entity e, float yaw, float pitch = 0, float roll = 0)
        { Begin(); Arg(e); Arg(yaw); Arg(pitch); Arg(roll); Unit("rotate"u8); }

        public Vector3 Scale(Entity e) { Begin(); Arg(e); Call("scale"u8); return OutVec(); }

        public void SetScale(Entity e, Vector3 s) { Begin(); Arg(e); Arg(s); Unit("set_scale"u8); }

        /// <summary>Hides or shows its mesh.</summary>
        public void SetHidden(Entity e, bool hidden) { Begin(); Arg(e); Arg(hidden); Unit("set_hidden"u8); }

        // ---- physics ------------------------------------------------

        /// <summary>Rigidbody velocity in m/s (zero without one).</summary>
        public Vector3 Velocity(Entity e) { Begin(); Arg(e); Call("velocity"u8); return OutVec(); }

        public void SetVelocity(Entity e, Vector3 v) { Begin(); Arg(e); Arg(v); Unit("set_velocity"u8); }

        /// <summary>An instant push (N·s).</summary>
        public void ApplyImpulse(Entity e, Vector3 i) { Begin(); Arg(e); Arg(i); Unit("apply_impulse"u8); }

        /// <summary>A force (N) for this tick.</summary>
        public void AddForce(Entity e, Vector3 f) { Begin(); Arg(e); Arg(f); Unit("add_force"u8); }

        /// <summary>The first solid collider along a ray, or null.</summary>
        public RaycastHit? Raycast(Vector3 origin, Vector3 direction, float max, Entity? ignore = null)
        {
            Begin();
            Arg(origin);
            Arg(direction);
            Arg(max);
            if (ignore is Entity skip) Arg(skip);
            Call("raycast"u8);
            return OutEntity(0) is Entity hit ? new RaycastHit(hit, OutNum(1)) : null;
        }

        // ---- sound and animation ------------------------------------

        public void PlaySound(Entity e) { Begin(); Arg(e); Unit("play_sound"u8); }

        public void StopSound(Entity e, float fade = 0) { Begin(); Arg(e); Arg(fade); Unit("stop_sound"u8); }

        public bool IsSoundPlaying(Entity e) { Begin(); Arg(e); Call("is_sound_playing"u8); return OutBool(0); }

        /// <summary>Plays a clip on its animator; false if there is no such clip.</summary>
        public bool PlayAnimation(Entity e, string clip, float fade = 0.2f, bool looping = true)
        {
            Begin();
            Arg(e);
            int at = count++;
            Arg(fade);
            Arg(looping);
            Call("play_animation"u8, clip, null, at);
            return OutBool(0);
        }

        public void StopAnimation(Entity e, float fade = 0.2f) { Begin(); Arg(e); Arg(fade); Unit("stop_animation"u8); }

        public bool IsAnimationPlaying(Entity e) { Begin(); Arg(e); Call("is_animation_playing"u8); return OutBool(0); }

        // ---- input --------------------------------------------------

        /// <summary>Whether an input action is held this tick.</summary>
        public bool Held(string action) { CallStr("held"u8, action); return OutBool(0); }

        /// <summary>Whether it was pressed this tick.</summary>
        public bool Pressed(string action) { CallStr("pressed"u8, action); return OutBool(0); }

        /// <summary>-1, 0 or 1 from two actions.</summary>
        public float Axis(string negative, string positive)
        {
            Begin();
            count = 2;
            Call("axis"u8, negative, positive, 0, 1);
            return OutNum(0);
        }

        // ---- messages, events, state and flow -----------------------

        /// <summary>Delivers a text message to the entity's behaviours after this phase.</summary>
        public void Send(Entity to, string message)
        {
            Begin();
            Arg(to);
            int at = count++;
            Call("send"u8, message, null, at);
        }

        /// <summary>Shows text on screen.</summary>
        public void ShowMessage(string text, float seconds = 3)
        {
            Begin();
            int at = count++;
            Arg(seconds);
            Call("show_message"u8, text, null, at);
        }

        /// <summary>Logs a game event, which headless test scripts can expect.</summary>
        public void Event(string name, string? detail = null)
        {
            Begin();
            count = detail == null ? 1 : 2;
            Call("event"u8, name, detail, 0, detail == null ? -1 : 1);
        }

        /// <summary>A value from the game state: bool, double, long, string or null.</summary>
        public object? GetState(string key)
        {
            CallStr("get_state"u8, key);
            return outs[0].Tag switch
            {
                Internal.NativeValue.TagBool => outs[0].I != 0,
                Internal.NativeValue.TagInt => outs[0].I,
                Internal.NativeValue.TagNum => outs[0].F,
                Internal.NativeValue.TagStr => outs[0].AsString(),
                _ => null,
            };
        }

        /// <summary>Stores a bool, number or string in the game state; null removes it.</summary>
        public void SetState(string key, object? value)
        {
            Begin();
            int at = count++;
            switch (value)
            {
                case null: args[count++] = default; break;
                case bool b: Arg(b); break;
                case int i: Arg((long)i); break;
                case long l: Arg(l); break;
                case float f: args[count++] = Internal.NativeValue.Num(f); break;
                case double d: args[count++] = Internal.NativeValue.Num(d); break;
                case string s:
                    count++;
                    Call("set_state"u8, key, s, at, at + 1);
                    return;
                default:
                    throw new ArgumentException($"game state holds bools, numbers and strings, not {value.GetType().Name}");
            }
            Call("set_state"u8, key, null, at);
        }

        /// <summary>Removes the entity after this phase.</summary>
        public void Despawn(Entity e) { Begin(); Arg(e); Unit("despawn"u8); }

        /// <summary>Loads another scene (project-relative) at the end of the tick.</summary>
        public void LoadScene(string path) => CallStr("load_scene"u8, path);

        /// <summary>Ends the game successfully.</summary>
        public void Finish(string message = "") => CallStr("finish"u8, message);

        /// <summary>Writes a line to the log.</summary>
        public void Log(string text) => CallStr("log"u8, text);
    }
}
