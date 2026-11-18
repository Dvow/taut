//! The built-in words in other languages (feature `translations`): German, Spanish, French,
//! Japanese, Portuguese and Simplified Chinese.
//!
//! A new [`Ui`] and [`FileWords::default()`] take the user's language (Windows: the display
//! language; Linux: `LANGUAGE`, else `LC_ALL`, `LC_MESSAGES`, `LANG`), except month and day
//! names and AM/PM, which follow the date formats ([`CalendarWords::of_the_system`]).
//! [`Ui::translate`] sets another.
use crate::date::CalendarWords;
use crate::file_dialog::FileWords;
use crate::popups::FrameWords;
use crate::ui::Ui;
use crate::web_widgets::WidgetWords;

/// How many words a language has: edit menu 6, search 1, code bars 11, frames 5, numbers 3,
/// web parts 10, clock 4, calendar arrows 2, file dialog 34, months 12, days 7, AM/PM 2.
const WORDS: usize = 97;
/// Where each kind of word starts.
const EDIT: usize = 0;
const SEARCH: usize = 6;
const CODE: usize = 7;
const FRAME: usize = 18;
const NUMBER: usize = 23;
const WIDGET: usize = 26;
const CLOCK: usize = 36;
const ARROWS: usize = 40;
const FILES: usize = 42;
const MONTHS: usize = 76;
const DAYS: usize = 88;
const HALVES: usize = 95;

/// Each language: its code, and its words.
#[rustfmt::skip]
const LANGUAGES: [(&str, [&str; WORDS]); 6] = [
    ("de", [
        "Ausschneiden", "Kopieren", "Einfügen", "Alles auswählen", "Zum Wörterbuch hinzufügen", "Ignorieren",
        "Suchen",
        "Suchen", "Ersetzen durch", "Groß/klein beachten", "Ganzes Wort", "Ersetzen", "Alle ersetzen", "Gehe zu Zeile", "von", "Weiter", "Zurück", "Schließen",
        "Mehr", "Schließen", "Dock füllen", "Zurück an seinen Platz", "Breite des Bereichs",
        "Keine Zahl", "Mindestens", "Höchstens",
        "Mehr", "Entfernen", "Schließen", "Erste Seite", "Vorherige Seite", "Nächste Seite", "Letzte Seite", "erledigt", "unteres Ende", "oberes Ende",
        "Stunden", "Minuten", "Sekunden", "AM oder PM",
        "Vorheriger Monat", "Nächster Monat",
        "Datei öffnen", "Dateien öffnen", "Speichern unter", "Ordner wählen", "Zurück", "Vorwärts", "Nach oben", "Ort", "Pfad eingeben", "Suchen",
        "Orte", "Persönlicher Ordner", "Zuletzt verwendet", "Laufwerke", "Neuer Ordner", "Ansicht", "Versteckte Dateien", "Kleine Bilder", "Abbrechen", "Öffnen",
        "Speichern", "Diesen Ordner wählen", "Der Name einer Datei", "Ein Ordner hierin, oder keiner für diesen selbst", "Name", "Größe", "Art", "Datum", "Ordner",
        "Ordner wird gelesen…", "Kein Name enthält das", "Nichts hat diesen Namen", "gibt es schon. Durch die neue ersetzen?", "Ersetzen",
        "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember",
        "Mo", "Di", "Mi", "Do", "Fr", "Sa", "So",
        "AM", "PM",
    ]),
    ("es", [
        "Cortar", "Copiar", "Pegar", "Seleccionar todo", "Añadir al diccionario", "Omitir",
        "Buscar",
        "Buscar", "Reemplazar por", "Mayúsculas y minúsculas", "Palabra completa", "Reemplazar", "Reemplazar todo", "Ir a la línea", "de", "Siguiente", "Anterior", "Cerrar",
        "Más", "Cerrar", "Llenar el panel", "Volver a su sitio", "Ancho del panel",
        "No es un número", "Como mínimo", "Como máximo",
        "Más", "Quitar", "Cerrar", "Primera página", "Página anterior", "Página siguiente", "Última página", "hecho", "extremo inferior", "extremo superior",
        "Horas", "Minutos", "Segundos", "a. m. o p. m.",
        "Mes anterior", "Mes siguiente",
        "Abrir un archivo", "Abrir archivos", "Guardar como", "Elegir una carpeta", "Atrás", "Adelante", "Subir", "Ubicación", "Escribir una ruta", "Buscar",
        "Lugares", "Carpeta personal", "Recientes", "Unidades", "Nueva carpeta", "Vista", "Archivos ocultos", "Miniaturas", "Cancelar", "Abrir",
        "Guardar", "Elegir esta carpeta", "El nombre de un archivo", "Una carpeta dentro de esta, o ninguna para esta misma", "Nombre", "Tamaño", "Tipo", "Fecha", "Carpeta",
        "Leyendo la carpeta…", "Ningún nombre contiene eso", "No hay nada con este nombre", "ya existe. ¿Reemplazarlo por el nuevo?", "Reemplazar",
        "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre",
        "lu", "ma", "mi", "ju", "vi", "sá", "do",
        "a. m.", "p. m.",
    ]),
    ("fr", [
        "Couper", "Copier", "Coller", "Tout sélectionner", "Ajouter au dictionnaire", "Ignorer",
        "Rechercher",
        "Rechercher", "Remplacer par", "Respecter la casse", "Mot entier", "Remplacer", "Tout remplacer", "Aller à la ligne", "sur", "Suivant", "Précédent", "Fermer",
        "Plus", "Fermer", "Remplir le dock", "Remettre à sa place", "Largeur du volet",
        "Pas un nombre", "Au moins", "Au plus",
        "Plus", "Retirer", "Fermer", "Première page", "Page précédente", "Page suivante", "Dernière page", "terminé", "borne basse", "borne haute",
        "Heures", "Minutes", "Secondes", "AM ou PM",
        "Mois précédent", "Mois suivant",
        "Ouvrir un fichier", "Ouvrir des fichiers", "Enregistrer sous", "Choisir un dossier", "Précédent", "Suivant", "Dossier parent", "Emplacement", "Saisir un chemin", "Rechercher",
        "Emplacements", "Dossier personnel", "Récents", "Lecteurs", "Nouveau dossier", "Affichage", "Fichiers cachés", "Miniatures", "Annuler", "Ouvrir",
        "Enregistrer", "Choisir ce dossier", "Le nom d’un fichier", "Un dossier dans celui-ci, ou aucun pour celui-ci", "Nom", "Taille", "Type", "Date", "Dossier",
        "Lecture du dossier…", "Aucun nom ne contient cela", "Rien ne porte ce nom", "existe déjà. Le remplacer par le nouveau ?", "Remplacer",
        "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre",
        "lu", "ma", "me", "je", "ve", "sa", "di",
        "AM", "PM",
    ]),
    ("ja", [
        "切り取り", "コピー", "貼り付け", "すべて選択", "辞書に追加", "無視",
        "検索",
        "検索", "置換後の文字列", "大文字と小文字を区別", "単語単位", "置換", "すべて置換", "行へ移動", "/", "次へ", "前へ", "閉じる",
        "その他", "閉じる", "ドック全体に表示", "元の位置に戻す", "ペインの幅",
        "数値ではありません", "最小", "最大",
        "その他", "削除", "閉じる", "最初のページ", "前のページ", "次のページ", "最後のページ", "完了", "下限", "上限",
        "時", "分", "秒", "午前または午後",
        "前の月", "次の月",
        "ファイルを開く", "複数のファイルを開く", "名前を付けて保存", "フォルダーの選択", "戻る", "進む", "上へ", "パス", "パスを入力", "検索",
        "場所", "ホーム", "最近使用した項目", "ドライブ", "新しいフォルダー", "表示", "隠しファイル", "縮小表示", "キャンセル", "開く",
        "保存", "このフォルダーを選択", "ファイル名", "このフォルダー内のフォルダー (空欄でこのフォルダー)", "名前", "サイズ", "種類", "日付", "フォルダー",
        "フォルダーを読み込んでいます…", "その名前を含むものはありません", "この名前のものはありません", "は既にあります。新しいもので置き換えますか?", "置き換える",
        "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
        "月", "火", "水", "木", "金", "土", "日",
        "午前", "午後",
    ]),
    ("pt", [
        "Recortar", "Copiar", "Colar", "Selecionar tudo", "Adicionar ao dicionário", "Ignorar",
        "Pesquisar",
        "Localizar", "Substituir por", "Diferenciar maiúsculas", "Palavra inteira", "Substituir", "Substituir tudo", "Ir para a linha", "de", "Próximo", "Anterior", "Fechar",
        "Mais", "Fechar", "Preencher o painel", "Voltar ao seu lugar", "Largura do painel",
        "Não é um número", "No mínimo", "No máximo",
        "Mais", "Remover", "Fechar", "Primeira página", "Página anterior", "Próxima página", "Última página", "concluído", "extremo inferior", "extremo superior",
        "Horas", "Minutos", "Segundos", "AM ou PM",
        "Mês anterior", "Próximo mês",
        "Abrir um arquivo", "Abrir arquivos", "Salvar como", "Escolher uma pasta", "Voltar", "Avançar", "Acima", "Local", "Digitar um caminho", "Pesquisar",
        "Locais", "Pasta pessoal", "Recentes", "Unidades", "Nova pasta", "Exibir", "Arquivos ocultos", "Miniaturas", "Cancelar", "Abrir",
        "Salvar", "Escolher esta pasta", "O nome de um arquivo", "Uma pasta dentro desta, ou nenhuma para esta mesma", "Nome", "Tamanho", "Tipo", "Data", "Pasta",
        "Lendo a pasta…", "Nenhum nome contém isso", "Não há nada com este nome", "já existe. Substituir pelo novo?", "Substituir",
        "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro",
        "seg", "ter", "qua", "qui", "sex", "sáb", "dom",
        "AM", "PM",
    ]),
    ("zh", [
        "剪切", "复制", "粘贴", "全选", "添加到词典", "忽略",
        "搜索",
        "查找", "替换为", "区分大小写", "全字匹配", "替换", "全部替换", "转到行", "/", "下一个", "上一个", "关闭",
        "更多", "关闭", "填满停靠区", "放回原处", "窗格宽度",
        "不是数字", "至少", "至多",
        "更多", "移除", "关闭", "第一页", "上一页", "下一页", "最后一页", "已完成", "下限", "上限",
        "时", "分", "秒", "上午或下午",
        "上个月", "下个月",
        "打开文件", "打开多个文件", "另存为", "选择文件夹", "后退", "前进", "向上", "路径", "输入路径", "搜索",
        "位置", "主文件夹", "最近", "驱动器", "新建文件夹", "视图", "隐藏文件", "缩略图", "取消", "打开",
        "保存", "选择此文件夹", "文件名", "此文件夹中的文件夹，留空则为此文件夹", "名称", "大小", "类型", "日期", "文件夹",
        "正在读取文件夹…", "没有名称包含该内容", "没有此名称的项目", "已存在。要用新的替换它吗？", "替换",
        "一月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "十一月", "十二月",
        "一", "二", "三", "四", "五", "六", "日",
        "上午", "下午",
    ]),
];

/// The words of `language` as the system names languages ("de", "pt-BR", "de_DE.UTF-8",
/// "zh-Hans-CN"), if it has a table: by its first part; Chinese only in simplified script.
fn words_of(language: &str) -> Option<&'static [&'static str; WORDS]> {
    let mut parts = language.split(['-', '_', '.', '@']);
    let first = parts.next()?.to_ascii_lowercase();
    let traditional = first == "zh" && parts.any(|part| ["tw", "hk", "mo", "hant"].contains(&part.to_ascii_lowercase().as_str()));
    LANGUAGES.iter().find(|(code, _)| *code == first && !traditional).map(|(_, words)| words)
}

/// The words of the first of the user's languages that has a table.
fn users_words() -> Option<&'static [&'static str; WORDS]> {
    users_languages().iter().find_map(|language| words_of(language))
}

/// A run of `N` words from `at`.
fn run<const N: usize>(words: &[&'static str; WORDS], at: usize) -> [&'static str; N] {
    std::array::from_fn(|nth| words[at + nth])
}

fn file_words(words: &[&'static str; WORDS]) -> FileWords {
    let [
        open_a_file,
        open_files,
        save_as,
        pick_a_folder,
        back,
        ahead,
        up,
        path,
        type_path,
        search,
        places,
        home,
        recent,
        drives,
        new_folder,
        view,
        hidden,
        grid,
        cancel,
        open,
        save,
        choose,
        name_hint,
        folder_hint,
        name,
        size,
        kind,
        date,
        folder,
        reading,
        none_found,
        not_there,
        there_already,
        replace,
    ] = run(words, FILES);
    FileWords {
        open_a_file,
        open_files,
        save_as,
        pick_a_folder,
        back,
        ahead,
        up,
        path,
        type_path,
        search,
        places,
        home,
        recent,
        drives,
        new_folder,
        view,
        hidden,
        grid,
        cancel,
        open,
        save,
        choose,
        name_hint,
        folder_hint,
        name,
        size,
        kind,
        date,
        folder,
        reading,
        none_found,
        not_there,
        there_already,
        replace,
    }
}

impl FileWords {
    /// The file dialog's words in `language` ("de", "fr-CA"), if taut has it.
    pub fn in_language(language: &str) -> Option<FileWords> {
        words_of(language).map(file_words)
    }
    /// The file dialog's words in the user's language; English if taut lacks it.
    pub(crate) fn users() -> FileWords {
        users_words().map_or_else(FileWords::english, file_words)
    }
}

/// The built-in words in other languages.
impl Ui {
    /// Sets every built-in word of this window to `language` as the system names it ("de",
    /// "pt-BR", "zh_CN"): menus, bars, number and web parts, calendar and clock, month and day
    /// names. Returns false, changing nothing, if taut lacks it. (A file dialog's words:
    /// [`FileWords::in_language`].)
    pub fn translate(&mut self, language: &str) -> bool {
        let Some(words) = words_of(language) else { return false };
        self.take_words(words);
        self.calendar_words.months = run(words, MONTHS);
        self.calendar_words.days = run(words, DAYS);
        self.calendar_words.halves = run(words, HALVES);
        true
    }
    /// Sets the built-in words to the user's language, if taut has it, but for month and day
    /// names and AM/PM (the system's, with its formats).
    pub(crate) fn translate_for_the_user(&mut self) {
        if let Some(words) = users_words() {
            self.take_words(words);
        }
    }
    fn take_words(&mut self, words: &[&'static str; WORDS]) {
        self.edit_words = run(words, EDIT);
        self.search_word = words[SEARCH];
        self.code_words = run(words, CODE);
        let [more, close, fill, back, splitter] = run(words, FRAME);
        self.frame_words = FrameWords { more, close, fill, back, splitter };
        self.number_words = run(words, NUMBER);
        let [more, remove, close, first, previous, next, last, done, low, high] = run(words, WIDGET);
        self.widget_words = WidgetWords { more, remove, close, first, previous, next, last, done, low, high };
        self.calendar_words = CalendarWords { clock: run(words, CLOCK), arrows: run(words, ARROWS), ..self.calendar_words };
    }
}

/// The user's display languages, most wanted first: Windows' list.
#[cfg(windows)]
fn users_languages() -> Vec<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserPreferredUILanguages(flags: u32, count: *mut u32, names: *mut u16, room: *mut u32) -> i32;
    }
    let (mut count, mut room, mut names) = (0u32, 512u32, [0u16; 512]);
    // MUI_LANGUAGE_NAME: names ("de-DE"), each ended by a NUL, the list by another. SAFETY: the
    // system writes at most `room` characters into `names`, which has that room.
    let said = unsafe { GetUserPreferredUILanguages(0x8, &mut count, names.as_mut_ptr(), &mut room) };
    let names = if said == 0 { &[][..] } else { &names[..(room as usize).min(names.len())] };
    names.split(|letter| *letter == 0).filter(|name| !name.is_empty()).map(String::from_utf16_lossy).collect()
}

/// The user's languages, most wanted first, as gettext looks for them: `LANGUAGE` (unless the
/// locale is C's), then the locale `LC_ALL`, `LC_MESSAGES` or `LANG` names.
#[cfg(not(windows))]
fn users_languages() -> Vec<String> {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let Some(locale) = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|name| var(name)) else { return Vec::new() };
    if matches!(locale.split(['.', '@']).next(), Some("C" | "POSIX")) {
        return Vec::new();
    }
    let mut languages: Vec<String> = var("LANGUAGE").map_or(Vec::new(), |list| list.split(':').map(str::to_owned).collect());
    languages.push(locale);
    languages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::LocalClipboard;
    use crate::text::BuiltInFont;

    #[test]
    fn every_language_has_every_word() {
        for (code, words) in LANGUAGES {
            assert!(words.iter().all(|word| !word.trim().is_empty()), "{code}");
            // No word left in English by mistake, but those English shares.
            let english = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
            let shared = ["AM", "PM", "Minutes", "Name", "Date", "Type", "Plus", "Local", "Abrir", "Data", "Tipo", "Pasta", "Ordner"];
            let same = english.edit_words.iter().chain(&english.code_words).filter(|word| words.contains(word) && !shared.contains(word)).count();
            assert_eq!(same, 0, "{code}");
            // Months, days and halves are where the calendar takes them.
            assert!(words[MONTHS..DAYS].iter().all(|month| month.chars().count() <= 10), "{code}");
        }
    }

    #[test]
    fn languages_are_found_by_the_names_systems_give_them() {
        let first_word = |language: &str| words_of(language).map(|words| words[0]);
        assert_eq!(
            ["de", "de-DE", "de_AT.UTF-8", "DE", "fr-CA", "pt-BR", "pt_PT", "ja-JP", "es-419"].map(first_word),
            [Some("Ausschneiden"), Some("Ausschneiden"), Some("Ausschneiden"), Some("Ausschneiden"), Some("Couper"), Some("Recortar"), Some("Recortar"), Some("切り取り"), Some("Cortar")]
        );
        assert_eq!(
            ["zh-CN", "zh_SG", "zh-Hans-CN", "zh-TW", "zh-Hant-HK", "zh_HK.UTF-8", "en-US", "", "xx"].map(first_word),
            [Some("剪切"), Some("剪切"), Some("剪切"), None, None, None, None, None, None]
        );
    }

    #[test]
    fn a_window_and_a_file_dialog_take_a_language() {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.edit_words = ["Cut", "Copy", "Paste", "Select all", "Add to dictionary", "Ignore"];
        assert!(!ui.translate("tlh"));
        assert_eq!(ui.edit_words[0], "Cut", "nothing changes for a language taut lacks");
        assert!(ui.translate("de-CH"));
        assert_eq!((ui.edit_words[2], ui.search_word, ui.code_words[10], ui.number_words[0]), ("Einfügen", "Suchen", "Schließen", "Keine Zahl"));
        assert_eq!((ui.frame_words.splitter, ui.widget_words.high, ui.calendar_words.clock[3], ui.calendar_words.arrows[1]), ("Breite des Bereichs", "oberes Ende", "AM oder PM", "Nächster Monat"));
        assert_eq!((ui.calendar_words.months[2], ui.calendar_words.days[6], ui.calendar_words.halves[1]), ("März", "So", "PM"));
        let french = FileWords::in_language("fr").unwrap();
        assert_eq!((french.open_a_file, french.replace, french.cancel), ("Ouvrir un fichier", "Remplacer", "Annuler"));
        assert_eq!(FileWords::in_language("en"), None);
    }

    /// The user's languages are read as gettext reads them: asked in a child process, so this
    /// test's own environment is untouched.
    #[cfg(not(windows))]
    #[test]
    fn the_users_language_comes_from_the_environment() {
        let asked = |names: &[(&str, &str)]| {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "translations::tests::the_users_words", "--ignored", "--nocapture"]);
            ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"].iter().for_each(|name| _ = child.env_remove(name));
            String::from_utf8_lossy(&child.envs(names.iter().copied()).output().unwrap().stdout).into_owned()
        };
        assert!(asked(&[("LANG", "de_DE.UTF-8")]).contains("[Ausschneiden|Ausschneiden|Datei öffnen]"));
        assert!(asked(&[("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "fr_FR.UTF-8")]).contains("[Couper|Couper|Ouvrir un fichier]"));
        assert!(asked(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "eo:ja:de")]).contains("[切り取り|切り取り|"), "the first with a table");
        assert!(asked(&[("LANG", "C.UTF-8"), ("LANGUAGE", "de")]).contains("[Cut|Cut|Open a file]"), "C's locale ignores LANGUAGE");
        assert!(asked(&[]).contains("[Cut|Cut|Open a file]"));
    }

    /// The child of the test above: prints what a new window and file dialog say.
    #[cfg(not(windows))]
    #[test]
    #[ignore]
    fn the_users_words() {
        let ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        println!("[{}|{}|{}]", users_words().map_or("Cut", |words| words[0]), ui.edit_words[0], FileWords::default().open_a_file);
    }

    /// Windows' display languages are read; the first with a table is taken.
    #[cfg(windows)]
    #[test]
    fn the_users_language_comes_from_windows() {
        let languages = users_languages();
        assert!(!languages.is_empty() && languages.iter().all(|name| name.len() >= 2 && name.is_ascii()), "{languages:?}");
        let ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        let wanted = languages.iter().find_map(|language| words_of(language)).map_or("Cut", |words| words[0]);
        assert_eq!(ui.edit_words[0], wanted);
    }
}
