//! The kaomoji, by category. Each entry is the face and the words a search
//! matches, space-separated.

pub(crate) struct Category {
    pub name: &'static str,
    pub faces: &'static [(&'static str, &'static str)],
}

pub(crate) const CATEGORIES: &[Category] = &[
    Category {
        name: "Happy",
        faces: &[
            ("(＾▽＾)", "happy smile grin"),
            ("(◕‿◕)", "happy smile cute"),
            ("(´｡• ᵕ •｡`)", "happy smile soft"),
            ("ヽ(・∀・)ﾉ", "happy cheer yay"),
            ("(≧◡≦)", "happy smile excited"),
            ("(*^‿^*)", "happy blush smile"),
            ("(o^▽^o)", "happy laugh"),
            ("(✿◠‿◠)", "happy flower cute"),
            ("٩(◕‿◕｡)۶", "happy cheer yay"),
            ("(ﾉ◕ヮ◕)ﾉ*:･ﾟ✧", "happy sparkle magic excited"),
            ("＼(＾▽＾)／", "happy cheer yay hooray"),
            ("(⌒‿⌒)", "happy content smile"),
            ("(￣▽￣)", "happy smug grin"),
            ("ᕕ( ᐛ )ᕗ", "happy walk strut"),
        ],
    },
    Category {
        name: "Love",
        faces: &[
            ("(♥ω♥*)", "love heart eyes"),
            ("(｡♥‿♥｡)", "love heart eyes crush"),
            ("(´ ω `♡)", "love heart"),
            ("(っ˘з(˘⌣˘ )", "love kiss"),
            ("(づ￣ ³￣)づ", "love kiss hug"),
            ("(*♡∀♡)", "love heart"),
            ("♡( ◡‿◡ )", "love heart content"),
            ("(灬º‿º灬)♡", "love blush heart"),
            ("( ˘ ³˘)♥", "love kiss heart"),
            ("(´,,•ω•,,)♡", "love shy heart"),
        ],
    },
    Category {
        name: "Hugs",
        faces: &[
            ("(っ´▽`)っ", "hug"),
            ("(づ｡◕‿‿◕｡)づ", "hug cute"),
            ("⊂(´• ω •`⊂)", "hug"),
            ("(つ≧▽≦)つ", "hug excited"),
            ("(っಠ‿ಠ)っ", "hug creepy"),
            ("༼ つ ◕_◕ ༽つ", "hug give gimme"),
            ("(⊃｡•́‿•̀｡)⊃", "hug offer"),
        ],
    },
    Category {
        name: "Sad",
        faces: &[
            ("(╥﹏╥)", "sad cry tears"),
            ("(ಥ﹏ಥ)", "sad cry tears"),
            ("(T_T)", "sad cry"),
            ("(；へ：)", "sad upset"),
            ("(っ˘̩╭╮˘̩)っ", "sad sniffle"),
            ("(｡•́︿•̀｡)", "sad pout"),
            ("(´；ω；`)", "sad cry"),
            ("( ´•̥̥̥ω•̥̥̥` )", "sad teary"),
            ("(ノ_<。)", "sad cry"),
            ("(｡╯︵╰｡)", "sad"),
        ],
    },
    Category {
        name: "Angry",
        faces: &[
            ("(╯°□°)╯︵ ┻━┻", "angry table flip rage"),
            ("┻━┻ ︵ヽ(`Д´)ﾉ︵ ┻━┻", "angry double table flip rage"),
            ("┬─┬ノ( º _ ºノ)", "table unflip calm put back"),
            ("(ノಠ益ಠ)ノ彡┻━┻", "angry table flip rage"),
            ("(＃`Д´)", "angry mad"),
            ("(╬ Ò﹏Ó)", "angry mad"),
            ("ヽ(`⌒´メ)ノ", "angry mad"),
            ("(¬_¬\")", "annoyed side eye"),
            ("(눈_눈)", "annoyed glare stare"),
            ("ಠ_ಠ", "disapproval look stare"),
            ("(ꐦ°᷄д°᷅)", "angry mad"),
        ],
    },
    Category {
        name: "Surprised",
        faces: &[
            ("(⊙_⊙)", "surprised shocked stare"),
            ("(°ロ°)", "surprised shocked"),
            ("Σ(°△°|||)", "surprised shocked gasp"),
            ("(ﾟДﾟ;)", "surprised shocked"),
            ("w(°ｏ°)w", "surprised wow"),
            ("(O_O;)", "surprised nervous"),
            ("(⊙ˍ⊙)", "surprised blank"),
            ("∑(O_O;)", "surprised shocked"),
        ],
    },
    Category {
        name: "Shrug",
        faces: &[
            (r"¯\_(ツ)_/¯", "shrug whatever dunno"),
            (r"┐(´д`)┌", "shrug whatever"),
            (r"ヽ(ー_ー )ノ", "shrug whatever"),
            (r"┐(￣ヘ￣)┌", "shrug whatever"),
            (r"╮(︶▽︶)╭", "shrug whatever"),
            (r"¯\_(°ペ)_/¯", "shrug confused"),
        ],
    },
    Category {
        name: "Embarrassed",
        faces: &[
            ("(⁄ ⁄•⁄ω⁄•⁄ ⁄)", "embarrassed blush shy"),
            ("(*/ω＼)", "embarrassed hide shy"),
            ("(〃▽〃)", "embarrassed blush"),
            ("(￣▽￣*)ゞ", "embarrassed sheepish"),
            ("(^_^;)", "embarrassed nervous sweat"),
            ("(；￣Д￣)", "embarrassed awkward"),
            ("(・_・;)", "nervous awkward sweat"),
        ],
    },
    Category {
        name: "Animals",
        faces: &[
            ("(=^･ω･^=)", "cat kitty"),
            ("(=①ω①=)", "cat kitty"),
            ("ฅ^•ﻌ•^ฅ", "cat paws kitty"),
            ("(^._.^)ﾉ", "cat wave"),
            ("U・ᴥ・U", "dog puppy"),
            ("∪･ω･∪", "dog puppy"),
            ("(ᵔᴥᵔ)", "bear"),
            ("ʕ •ᴥ•ʔ", "bear"),
            ("ʕっ•ᴥ•ʔっ", "bear hug"),
            ("/(・ × ・)\\", "bunny rabbit"),
            ("(•ө•)♡", "bird chick"),
            ("<°)))彡", "fish"),
            ("くコ:彡", "squid"),
        ],
    },
    Category {
        name: "Greetings",
        faces: &[
            ("(・ω・)ノ", "hi hello wave"),
            ("ヾ(＾∇＾)", "hi hello wave"),
            ("(￣▽￣)ノ", "hi hello wave"),
            ("(^-^*)/", "hi hello bye wave"),
            ("(*・ω・)ﾉ", "hi hello wave"),
            ("(｡･ω･)ﾉﾞ", "bye goodbye wave"),
            ("m(_ _)m", "bow sorry thanks"),
            ("(￣^￣)ゞ", "salute"),
        ],
    },
    Category {
        name: "Sleepy",
        faces: &[
            ("(－_－) zzZ", "sleepy sleep tired zzz"),
            ("(∪｡∪)｡｡｡zzZ", "sleepy sleep zzz"),
            ("(´〜｀*) zzz", "sleepy tired yawn"),
            ("(x_x)", "dead tired ko"),
            ("(￣o￣) zzZZzzZZ", "sleepy snore"),
        ],
    },
    Category {
        name: "Actions",
        faces: &[
            ("( ͡° ͜ʖ ͡°)", "lenny smirk"),
            ("(☞ﾟヮﾟ)☞", "point you"),
            ("☜(ﾟヮﾟ☜)", "point you"),
            ("(•_•) ( •_•)>⌐■-■ (⌐■_■)", "sunglasses deal with it cool"),
            ("(ง'̀-'́)ง", "fight punch"),
            ("(ﾉ´ヮ`)ﾉ*: ･ﾟ", "sparkle magic throw"),
            ("( ˘▽˘)っ♨", "tea coffee hot drink"),
            ("(っ˘ڡ˘ς)", "eat yum food"),
            ("♪(´ε｀ )", "music sing whistle"),
            ("ヾ(⌐■_■)ノ♪", "dance music cool"),
            ("(｀・ω・´)ゞ", "salute ready determined"),
            ("φ(..)", "write note"),
            ("(✧ω✧)", "sparkle eyes"),
            ("(ಠ‿ಠ)", "evil scheming"),
        ],
    },
];

/// Every face, in category order: (category index, face, words).
pub(crate) fn all() -> impl Iterator<Item = (usize, &'static str, &'static str)> {
    CATEGORIES
        .iter()
        .enumerate()
        .flat_map(|(c, cat)| cat.faces.iter().map(move |(f, w)| (c, *f, *w)))
}

/// Faces matching `query`: every query word starts a word of the entry or
/// its category name.
pub(crate) fn search(query: &str) -> Vec<&'static str> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    all()
        .filter(|(c, _, tags)| {
            let cat = CATEGORIES[*c].name.to_lowercase();
            words.iter().all(|q| {
                tags.split(' ')
                    .chain(std::iter::once(cat.as_str()))
                    .any(|t| t.starts_with(q.as_str()))
            })
        })
        .map(|(_, f, _)| f)
        .collect()
}
