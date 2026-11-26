//! The built-in words in other languages (feature `translations`): Arabic, Chinese (simplified
//! and traditional), Dutch, French, German, Hindi, Indonesian, Italian, Japanese, Korean,
//! Polish, Portuguese, Russian, Spanish, Swedish, Thai, Turkish, Ukrainian and Vietnamese. Month
//! and day names and AM/PM are CLDR's.
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

/// How many words a language has: edit menu 7, search 1, code bars 11, frames 5, numbers 3,
/// web parts 10, clock 4, calendar arrows 2, file dialog 34, months 12, days 7, AM/PM 2.
const WORDS: usize = 98;
/// Where each kind of word starts.
const EDIT: usize = 0;
const SEARCH: usize = 7;
const CODE: usize = 8;
const FRAME: usize = 19;
const NUMBER: usize = 24;
const WIDGET: usize = 27;
const CLOCK: usize = 37;
const ARROWS: usize = 41;
const FILES: usize = 43;
const MONTHS: usize = 77;
const DAYS: usize = 89;
const HALVES: usize = 96;

/// Each language: its code ("zh-Hant" for traditional Chinese), and its words (`split_words`).
#[rustfmt::skip]
static LANGUAGES: [(&str, &str); 20] = [
    ("de", concat!(
        "Ausschneiden|Kopieren|Einfügen|Alles auswählen|Zum Wörterbuch hinzufügen|Ignorieren|Link kopieren|",
        "Suchen|",
        "Suchen|Ersetzen durch|Groß/klein beachten|Ganzes Wort|Ersetzen|Alle ersetzen|Gehe zu Zeile|von|Weiter|Zurück|Schließen|",
        "Mehr|Schließen|Dock füllen|Zurück an seinen Platz|Breite des Bereichs|",
        "Keine Zahl|Mindestens|Höchstens|",
        "Mehr|Entfernen|Schließen|Erste Seite|Vorherige Seite|Nächste Seite|Letzte Seite|erledigt|unteres Ende|oberes Ende|",
        "Stunden|Minuten|Sekunden|AM oder PM|",
        "Vorheriger Monat|Nächster Monat|",
        "Datei öffnen|Dateien öffnen|Speichern unter|Ordner wählen|Zurück|Vorwärts|Nach oben|Ort|Pfad eingeben|Suchen|",
        "Orte|Persönlicher Ordner|Zuletzt verwendet|Laufwerke|Neuer Ordner|Ansicht|Versteckte Dateien|Kleine Bilder|Abbrechen|Öffnen|",
        "Speichern|Diesen Ordner wählen|Der Name einer Datei|Ein Ordner hierin, oder keiner für diesen selbst|Name|Größe|Art|Datum|Ordner|",
        "Ordner wird gelesen…|Kein Name enthält das|Nichts hat diesen Namen|gibt es schon. Durch die neue ersetzen?|Ersetzen|",
        "Januar|Februar|März|April|Mai|Juni|Juli|August|September|Oktober|November|Dezember|",
        "Mo|Di|Mi|Do|Fr|Sa|So|",
        "AM|PM|",
    )),
    ("es", concat!(
        "Cortar|Copiar|Pegar|Seleccionar todo|Añadir al diccionario|Omitir|Copiar enlace|",
        "Buscar|",
        "Buscar|Reemplazar por|Mayúsculas y minúsculas|Palabra completa|Reemplazar|Reemplazar todo|Ir a la línea|de|Siguiente|Anterior|Cerrar|",
        "Más|Cerrar|Llenar el panel|Volver a su sitio|Ancho del panel|",
        "No es un número|Como mínimo|Como máximo|",
        "Más|Quitar|Cerrar|Primera página|Página anterior|Página siguiente|Última página|hecho|extremo inferior|extremo superior|",
        "Horas|Minutos|Segundos|a. m. o p. m.|",
        "Mes anterior|Mes siguiente|",
        "Abrir un archivo|Abrir archivos|Guardar como|Elegir una carpeta|Atrás|Adelante|Subir|Ubicación|Escribir una ruta|Buscar|",
        "Lugares|Carpeta personal|Recientes|Unidades|Nueva carpeta|Vista|Archivos ocultos|Miniaturas|Cancelar|Abrir|",
        "Guardar|Elegir esta carpeta|El nombre de un archivo|Una carpeta dentro de esta, o ninguna para esta misma|Nombre|Tamaño|Tipo|Fecha|Carpeta|",
        "Leyendo la carpeta…|Ningún nombre contiene eso|No hay nada con este nombre|ya existe. ¿Reemplazarlo por el nuevo?|Reemplazar|",
        "enero|febrero|marzo|abril|mayo|junio|julio|agosto|septiembre|octubre|noviembre|diciembre|",
        "lu|ma|mi|ju|vi|sá|do|",
        "a. m.|p. m.|",
    )),
    ("fr", concat!(
        "Couper|Copier|Coller|Tout sélectionner|Ajouter au dictionnaire|Ignorer|Copier le lien|",
        "Rechercher|",
        "Rechercher|Remplacer par|Respecter la casse|Mot entier|Remplacer|Tout remplacer|Aller à la ligne|sur|Suivant|Précédent|Fermer|",
        "Plus|Fermer|Remplir le dock|Remettre à sa place|Largeur du volet|",
        "Pas un nombre|Au moins|Au plus|",
        "Plus|Retirer|Fermer|Première page|Page précédente|Page suivante|Dernière page|terminé|borne basse|borne haute|",
        "Heures|Minutes|Secondes|AM ou PM|",
        "Mois précédent|Mois suivant|",
        "Ouvrir un fichier|Ouvrir des fichiers|Enregistrer sous|Choisir un dossier|Précédent|Suivant|Dossier parent|Emplacement|Saisir un chemin|Rechercher|",
        "Emplacements|Dossier personnel|Récents|Lecteurs|Nouveau dossier|Affichage|Fichiers cachés|Miniatures|Annuler|Ouvrir|",
        "Enregistrer|Choisir ce dossier|Le nom d’un fichier|Un dossier dans celui-ci, ou aucun pour celui-ci|Nom|Taille|Type|Date|Dossier|",
        "Lecture du dossier…|Aucun nom ne contient cela|Rien ne porte ce nom|existe déjà. Le remplacer par le nouveau ?|Remplacer|",
        "janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre|",
        "lu|ma|me|je|ve|sa|di|",
        "AM|PM|",
    )),
    ("ja", concat!(
        "切り取り|コピー|貼り付け|すべて選択|辞書に追加|無視|リンクをコピー|",
        "検索|",
        "検索|置換後の文字列|大文字と小文字を区別|単語単位|置換|すべて置換|行へ移動|/|次へ|前へ|閉じる|",
        "その他|閉じる|ドック全体に表示|元の位置に戻す|ペインの幅|",
        "数値ではありません|最小|最大|",
        "その他|削除|閉じる|最初のページ|前のページ|次のページ|最後のページ|完了|下限|上限|",
        "時|分|秒|午前または午後|",
        "前の月|次の月|",
        "ファイルを開く|複数のファイルを開く|名前を付けて保存|フォルダーの選択|戻る|進む|上へ|パス|パスを入力|検索|",
        "場所|ホーム|最近使用した項目|ドライブ|新しいフォルダー|表示|隠しファイル|縮小表示|キャンセル|開く|",
        "保存|このフォルダーを選択|ファイル名|このフォルダー内のフォルダー (空欄でこのフォルダー)|名前|サイズ|種類|日付|フォルダー|",
        "フォルダーを読み込んでいます…|その名前を含むものはありません|この名前のものはありません|は既にあります。新しいもので置き換えますか?|置き換える|",
        "1月|2月|3月|4月|5月|6月|7月|8月|9月|10月|11月|12月|",
        "月|火|水|木|金|土|日|",
        "午前|午後|",
    )),
    ("pt", concat!(
        "Recortar|Copiar|Colar|Selecionar tudo|Adicionar ao dicionário|Ignorar|Copiar link|",
        "Pesquisar|",
        "Localizar|Substituir por|Diferenciar maiúsculas|Palavra inteira|Substituir|Substituir tudo|Ir para a linha|de|Próximo|Anterior|Fechar|",
        "Mais|Fechar|Preencher o painel|Voltar ao seu lugar|Largura do painel|",
        "Não é um número|No mínimo|No máximo|",
        "Mais|Remover|Fechar|Primeira página|Página anterior|Próxima página|Última página|concluído|extremo inferior|extremo superior|",
        "Horas|Minutos|Segundos|AM ou PM|",
        "Mês anterior|Próximo mês|",
        "Abrir um arquivo|Abrir arquivos|Salvar como|Escolher uma pasta|Voltar|Avançar|Acima|Local|Digitar um caminho|Pesquisar|",
        "Locais|Pasta pessoal|Recentes|Unidades|Nova pasta|Exibir|Arquivos ocultos|Miniaturas|Cancelar|Abrir|",
        "Salvar|Escolher esta pasta|O nome de um arquivo|Uma pasta dentro desta, ou nenhuma para esta mesma|Nome|Tamanho|Tipo|Data|Pasta|",
        "Lendo a pasta…|Nenhum nome contém isso|Não há nada com este nome|já existe. Substituir pelo novo?|Substituir|",
        "janeiro|fevereiro|março|abril|maio|junho|julho|agosto|setembro|outubro|novembro|dezembro|",
        "seg|ter|qua|qui|sex|sáb|dom|",
        "AM|PM|",
    )),
    ("zh", concat!(
        "剪切|复制|粘贴|全选|添加到词典|忽略|复制链接|",
        "搜索|",
        "查找|替换为|区分大小写|全字匹配|替换|全部替换|转到行|/|下一个|上一个|关闭|",
        "更多|关闭|填满停靠区|放回原处|窗格宽度|",
        "不是数字|至少|至多|",
        "更多|移除|关闭|第一页|上一页|下一页|最后一页|已完成|下限|上限|",
        "时|分|秒|上午或下午|",
        "上个月|下个月|",
        "打开文件|打开多个文件|另存为|选择文件夹|后退|前进|向上|路径|输入路径|搜索|",
        "位置|主文件夹|最近|驱动器|新建文件夹|视图|隐藏文件|缩略图|取消|打开|",
        "保存|选择此文件夹|文件名|此文件夹中的文件夹，留空则为此文件夹|名称|大小|类型|日期|文件夹|",
        "正在读取文件夹…|没有名称包含该内容|没有此名称的项目|已存在。要用新的替换它吗？|替换|",
        "一月|二月|三月|四月|五月|六月|七月|八月|九月|十月|十一月|十二月|",
        "一|二|三|四|五|六|日|",
        "上午|下午|",
    )),
    ("zh-Hant", concat!(
        "剪下|複製|貼上|全選|新增至字典|略過|複製連結|",
        "搜尋|",
        "尋找|取代為|大小寫須相符|全字拼寫須相符|取代|全部取代|移至行|/|下一個|上一個|關閉|",
        "更多|關閉|填滿停駐區|回到原位|窗格寬度|",
        "不是數字|至少|至多|",
        "更多|移除|關閉|第一頁|上一頁|下一頁|最後一頁|完成|下限|上限|",
        "時|分|秒|上午或下午|",
        "上個月|下個月|",
        "開啟檔案|開啟多個檔案|另存新檔|選擇資料夾|返回|前進|向上|路徑|輸入路徑|搜尋|",
        "位置|個人資料夾|最近|磁碟機|新增資料夾|檢視|隱藏檔案|縮圖|取消|開啟|",
        "儲存|選擇此資料夾|檔案名稱|此資料夾中的資料夾，留空則為此資料夾|名稱|大小|類型|日期|資料夾|",
        "正在讀取資料夾…|沒有名稱包含該內容|沒有此名稱的項目|已存在。要以新的取代嗎？|取代|",
        "1月|2月|3月|4月|5月|6月|7月|8月|9月|10月|11月|12月|",
        "一|二|三|四|五|六|日|",
        "上午|下午|",
    )),
    ("it", concat!(
        "Taglia|Copia|Incolla|Seleziona tutto|Aggiungi al dizionario|Ignora|Copia link|",
        "Cerca|",
        "Trova|Sostituisci con|Maiuscole/minuscole|Parola intera|Sostituisci|Sostituisci tutto|Vai alla riga|di|Successivo|Precedente|Chiudi|",
        "Altro|Chiudi|Riempi l’area di ancoraggio|Torna al suo posto|Larghezza del riquadro|",
        "Non è un numero|Almeno|Al massimo|",
        "Altro|Rimuovi|Chiudi|Prima pagina|Pagina precedente|Pagina successiva|Ultima pagina|completato|estremo inferiore|estremo superiore|",
        "Ore|Minuti|Secondi|AM o PM|",
        "Mese precedente|Mese successivo|",
        "Apri un file|Apri file|Salva con nome|Scegli una cartella|Indietro|Avanti|Su|Percorso|Digita un percorso|Cerca|",
        "Posizioni|Cartella personale|Recenti|Unità|Nuova cartella|Visualizza|File nascosti|Miniature|Annulla|Apri|",
        "Salva|Scegli questa cartella|Il nome di un file|Una cartella in questa, o nessuna per questa stessa|Nome|Dimensione|Tipo|Data|Cartella|",
        "Lettura della cartella…|Nessun nome contiene questo testo|Non c’è nulla con questo nome|esiste già. Sostituirlo con il nuovo?|Sostituisci|",
        "gennaio|febbraio|marzo|aprile|maggio|giugno|luglio|agosto|settembre|ottobre|novembre|dicembre|",
        "lun|mar|mer|gio|ven|sab|dom|",
        "AM|PM|",
    )),
    ("ru", concat!(
        "Вырезать|Копировать|Вставить|Выделить все|Добавить в словарь|Пропустить|Копировать ссылку|",
        "Поиск|",
        "Найти|Заменить на|С учетом регистра|Слово целиком|Заменить|Заменить все|Перейти к строке|из|Далее|Назад|Закрыть|",
        "Еще|Закрыть|Заполнить область закрепления|Вернуть на место|Ширина панели|",
        "Не число|Не меньше|Не больше|",
        "Еще|Удалить|Закрыть|Первая страница|Предыдущая страница|Следующая страница|Последняя страница|готово|нижняя граница|верхняя граница|",
        "Часы|Минуты|Секунды|AM или PM|",
        "Предыдущий месяц|Следующий месяц|",
        "Открыть файл|Открыть файлы|Сохранить как|Выбрать папку|Назад|Вперед|Вверх|Расположение|Введите путь|Поиск|",
        "Места|Домашняя папка|Недавние|Диски|Новая папка|Вид|Скрытые файлы|Эскизы|Отмена|Открыть|",
        "Сохранить|Выбрать эту папку|Имя файла|Папка внутри этой или никакой для нее самой|Имя|Размер|Тип|Дата|Папка|",
        "Чтение папки…|Ни одно имя не содержит этого|Ничего с таким именем нет|уже существует. Заменить новым?|Заменить|",
        "январь|февраль|март|апрель|май|июнь|июль|август|сентябрь|октябрь|ноябрь|декабрь|",
        "пн|вт|ср|чт|пт|сб|вс|",
        "AM|PM|",
    )),
    ("ko", concat!(
        "잘라내기|복사|붙여넣기|모두 선택|사전에 추가|무시|링크 복사|",
        "검색|",
        "찾기|바꿀 내용|대/소문자 구분|단어 단위로|바꾸기|모두 바꾸기|줄로 이동|/|다음|이전|닫기|",
        "더 보기|닫기|도킹 영역 채우기|원래 위치로|창 너비|",
        "숫자가 아님|최소|최대|",
        "더 보기|제거|닫기|첫 페이지|이전 페이지|다음 페이지|마지막 페이지|완료|하한|상한|",
        "시|분|초|오전 또는 오후|",
        "이전 달|다음 달|",
        "파일 열기|여러 파일 열기|다른 이름으로 저장|폴더 선택|뒤로|앞으로|위로|위치|경로 입력|검색|",
        "장소|홈|최근 항목|드라이브|새 폴더|보기|숨김 파일|축소판 그림|취소|열기|",
        "저장|이 폴더 선택|파일 이름|이 폴더 안의 폴더 (비워 두면 이 폴더)|이름|크기|종류|날짜|폴더|",
        "폴더를 읽는 중…|그 내용이 들어간 이름이 없습니다|이 이름을 가진 항목이 없습니다|이(가) 이미 있습니다. 새 항목으로 바꾸시겠습니까?|바꾸기|",
        "1월|2월|3월|4월|5월|6월|7월|8월|9월|10월|11월|12월|",
        "월|화|수|목|금|토|일|",
        "오전|오후|",
    )),
    ("ar", concat!(
        "قص|نسخ|لصق|تحديد الكل|إضافة إلى القاموس|تجاهل|نسخ الرابط|",
        "بحث|",
        "بحث|استبدال بـ|مطابقة حالة الأحرف|الكلمة بأكملها|استبدال|استبدال الكل|الانتقال إلى السطر|من|التالي|السابق|إغلاق|",
        "المزيد|إغلاق|ملء منطقة الإرساء|إعادة إلى مكانه|عرض الجزء|",
        "ليس رقمًا|على الأقل|على الأكثر|",
        "المزيد|إزالة|إغلاق|الصفحة الأولى|الصفحة السابقة|الصفحة التالية|الصفحة الأخيرة|تم|الحد الأدنى|الحد الأعلى|",
        "الساعات|الدقائق|الثواني|ص أو م|",
        "الشهر السابق|الشهر التالي|",
        "فتح ملف|فتح ملفات|حفظ باسم|اختيار مجلد|رجوع|تقدم|لأعلى|الموقع|اكتب مسارًا|بحث|",
        "الأماكن|المجلد الرئيسي|الأخيرة|محركات الأقراص|مجلد جديد|عرض|الملفات المخفية|صور مصغرة|إلغاء|فتح|",
        "حفظ|اختيار هذا المجلد|اسم ملف|مجلد داخل هذا المجلد، أو لا شيء لهذا المجلد نفسه|الاسم|الحجم|النوع|التاريخ|مجلد|",
        "جارٍ قراءة المجلد…|لا يوجد اسم يحتوي على ذلك|لا يوجد شيء بهذا الاسم|موجود بالفعل. هل تريد استبداله بالجديد؟|استبدال|",
        "يناير|فبراير|مارس|أبريل|مايو|يونيو|يوليو|أغسطس|سبتمبر|أكتوبر|نوفمبر|ديسمبر|",
        "ن|ث|ر|خ|ج|س|ح|",
        "ص|م|",
    )),
    ("hi", concat!(
        "काटें|कॉपी करें|चिपकाएँ|सभी चुनें|शब्दकोश में जोड़ें|अनदेखा करें|लिंक कॉपी करें|",
        "खोजें|",
        "ढूँढें|इससे बदलें|केस मिलाएँ|पूरा शब्द|बदलें|सभी बदलें|पंक्ति पर जाएँ|में से|अगला|पिछला|बंद करें|",
        "और|बंद करें|डॉक भरें|अपनी जगह पर वापस|फलक की चौड़ाई|",
        "संख्या नहीं है|कम से कम|अधिक से अधिक|",
        "और|निकालें|बंद करें|पहला पृष्ठ|पिछला पृष्ठ|अगला पृष्ठ|अंतिम पृष्ठ|पूर्ण|निचली सीमा|ऊपरी सीमा|",
        "घंटे|मिनट|सेकंड|am या pm|",
        "पिछला महीना|अगला महीना|",
        "फ़ाइल खोलें|फ़ाइलें खोलें|इस रूप में सहेजें|फ़ोल्डर चुनें|वापस|आगे|ऊपर|स्थान|पथ लिखें|खोजें|",
        "जगहें|होम|हाल के|ड्राइव|नया फ़ोल्डर|दृश्य|छिपी फ़ाइलें|थंबनेल|रद्द करें|खोलें|",
        "सहेजें|यह फ़ोल्डर चुनें|किसी फ़ाइल का नाम|इसके अंदर का कोई फ़ोल्डर, या इसी के लिए कोई नहीं|नाम|आकार|प्रकार|दिनांक|फ़ोल्डर|",
        "फ़ोल्डर पढ़ा जा रहा है…|किसी नाम में यह नहीं है|इस नाम का कुछ नहीं है|पहले से मौजूद है। क्या इसे नए से बदलें?|बदलें|",
        "जनवरी|फ़रवरी|मार्च|अप्रैल|मई|जून|जुलाई|अगस्त|सितंबर|अक्टूबर|नवंबर|दिसंबर|",
        "सो|मं|बु|गु|शु|श|र|",
        "am|pm|",
    )),
    ("tr", concat!(
        "Kes|Kopyala|Yapıştır|Tümünü seç|Sözlüğe ekle|Yoksay|Bağlantıyı kopyala|",
        "Ara|",
        "Bul|Şununla değiştir|Büyük/küçük harf eşleştir|Tam sözcük|Değiştir|Tümünü değiştir|Satıra git|/|Sonraki|Önceki|Kapat|",
        "Diğer|Kapat|Yerleştirme alanını doldur|Yerine geri koy|Bölme genişliği|",
        "Sayı değil|En az|En çok|",
        "Diğer|Kaldır|Kapat|İlk sayfa|Önceki sayfa|Sonraki sayfa|Son sayfa|tamamlandı|alt sınır|üst sınır|",
        "Saat|Dakika|Saniye|ÖÖ veya ÖS|",
        "Önceki ay|Sonraki ay|",
        "Dosya aç|Dosyaları aç|Farklı kaydet|Klasör seç|Geri|İleri|Yukarı|Konum|Yol yazın|Ara|",
        "Yerler|Ana klasör|Son kullanılanlar|Sürücüler|Yeni klasör|Görünüm|Gizli dosyalar|Küçük resimler|İptal|Aç|",
        "Kaydet|Bu klasörü seç|Bir dosyanın adı|Bunun içinde bir klasör ya da bunun kendisi için hiçbiri|Ad|Boyut|Tür|Tarih|Klasör|",
        "Klasör okunuyor…|Adında bu geçen bir şey yok|Bu adda bir şey yok|zaten var. Yenisiyle değiştirilsin mi?|Değiştir|",
        "Ocak|Şubat|Mart|Nisan|Mayıs|Haziran|Temmuz|Ağustos|Eylül|Ekim|Kasım|Aralık|",
        "Pt|Sa|Ça|Pe|Cu|Ct|Pa|",
        "ÖÖ|ÖS|",
    )),
    ("pl", concat!(
        "Wytnij|Kopiuj|Wklej|Zaznacz wszystko|Dodaj do słownika|Ignoruj|Kopiuj link|",
        "Szukaj|",
        "Znajdź|Zamień na|Uwzględniaj wielkość liter|Całe wyrazy|Zamień|Zamień wszystko|Przejdź do wiersza|z|Następny|Poprzedni|Zamknij|",
        "Więcej|Zamknij|Wypełnij obszar dokowania|Wróć na miejsce|Szerokość panelu|",
        "To nie jest liczba|Co najmniej|Co najwyżej|",
        "Więcej|Usuń|Zamknij|Pierwsza strona|Poprzednia strona|Następna strona|Ostatnia strona|gotowe|dolny koniec|górny koniec|",
        "Godziny|Minuty|Sekundy|AM lub PM|",
        "Poprzedni miesiąc|Następny miesiąc|",
        "Otwórz plik|Otwórz pliki|Zapisz jako|Wybierz folder|Wstecz|Dalej|W górę|Lokalizacja|Wpisz ścieżkę|Szukaj|",
        "Miejsca|Folder domowy|Ostatnie|Dyski|Nowy folder|Widok|Ukryte pliki|Miniatury|Anuluj|Otwórz|",
        "Zapisz|Wybierz ten folder|Nazwa pliku|Folder w tym folderze lub żaden dla niego samego|Nazwa|Rozmiar|Typ|Data|Folder|",
        "Odczytywanie folderu…|Żadna nazwa tego nie zawiera|Nie ma niczego o tej nazwie|już istnieje. Zastąpić nowym?|Zastąp|",
        "styczeń|luty|marzec|kwiecień|maj|czerwiec|lipiec|sierpień|wrzesień|październik|listopad|grudzień|",
        "pon|wto|śro|czw|pią|sob|nie|",
        "AM|PM|",
    )),
    ("nl", concat!(
        "Knippen|Kopiëren|Plakken|Alles selecteren|Toevoegen aan woordenlijst|Negeren|Link kopiëren|",
        "Zoeken|",
        "Zoeken|Vervangen door|Hoofdlettergevoelig|Heel woord|Vervangen|Alles vervangen|Ga naar regel|van|Volgende|Vorige|Sluiten|",
        "Meer|Sluiten|Dockgebied vullen|Terug naar zijn plaats|Breedte van het deelvenster|",
        "Geen getal|Minstens|Hoogstens|",
        "Meer|Verwijderen|Sluiten|Eerste pagina|Vorige pagina|Volgende pagina|Laatste pagina|klaar|ondergrens|bovengrens|",
        "Uren|Minuten|Seconden|a.m. of p.m.|",
        "Vorige maand|Volgende maand|",
        "Een bestand openen|Bestanden openen|Opslaan als|Een map kiezen|Terug|Vooruit|Omhoog|Locatie|Typ een pad|Zoeken|",
        "Locaties|Persoonlijke map|Recent|Stations|Nieuwe map|Weergave|Verborgen bestanden|Miniaturen|Annuleren|Openen|",
        "Opslaan|Deze map kiezen|De naam van een bestand|Een map hierin, of geen voor deze map zelf|Naam|Grootte|Soort|Datum|Map|",
        "Map wordt gelezen…|Geen naam bevat dat|Er is niets met deze naam|bestaat al. Vervangen door het nieuwe?|Vervangen|",
        "januari|februari|maart|april|mei|juni|juli|augustus|september|oktober|november|december|",
        "ma|di|wo|do|vr|za|zo|",
        "a.m.|p.m.|",
    )),
    ("id", concat!(
        "Potong|Salin|Tempel|Pilih semua|Tambahkan ke kamus|Abaikan|Salin tautan|",
        "Cari|",
        "Temukan|Ganti dengan|Cocokkan huruf besar/kecil|Seluruh kata|Ganti|Ganti semua|Buka baris|dari|Berikutnya|Sebelumnya|Tutup|",
        "Lainnya|Tutup|Isi area dok|Kembali ke tempatnya|Lebar panel|",
        "Bukan angka|Minimal|Maksimal|",
        "Lainnya|Hapus|Tutup|Halaman pertama|Halaman sebelumnya|Halaman berikutnya|Halaman terakhir|selesai|batas bawah|batas atas|",
        "Jam|Menit|Detik|AM atau PM|",
        "Bulan sebelumnya|Bulan berikutnya|",
        "Buka file|Buka beberapa file|Simpan sebagai|Pilih folder|Kembali|Maju|Ke atas|Lokasi|Ketik jalur|Cari|",
        "Tempat|Folder rumah|Terbaru|Drive|Folder baru|Tampilan|File tersembunyi|Gambar mini|Batal|Buka|",
        "Simpan|Pilih folder ini|Nama file|Folder di dalam folder ini, atau kosong untuk folder ini sendiri|Nama|Ukuran|Jenis|Tanggal|Folder|",
        "Membaca folder…|Tidak ada nama yang memuat itu|Tidak ada apa pun dengan nama ini|sudah ada. Ganti dengan yang baru?|Ganti|",
        "Januari|Februari|Maret|April|Mei|Juni|Juli|Agustus|September|Oktober|November|Desember|",
        "Sen|Sel|Rab|Kam|Jum|Sab|Min|",
        "AM|PM|",
    )),
    ("vi", concat!(
        "Cắt|Sao chép|Dán|Chọn tất cả|Thêm vào từ điển|Bỏ qua|Sao chép liên kết|",
        "Tìm kiếm|",
        "Tìm|Thay bằng|Khớp chữ hoa/thường|Toàn bộ từ|Thay thế|Thay thế tất cả|Đi tới dòng|/|Tiếp|Trước|Đóng|",
        "Thêm|Đóng|Lấp đầy khu neo|Trả về chỗ cũ|Độ rộng ngăn|",
        "Không phải số|Ít nhất|Nhiều nhất|",
        "Thêm|Xóa|Đóng|Trang đầu|Trang trước|Trang sau|Trang cuối|xong|đầu dưới|đầu trên|",
        "Giờ|Phút|Giây|SA hoặc CH|",
        "Tháng trước|Tháng sau|",
        "Mở tệp|Mở các tệp|Lưu thành|Chọn thư mục|Quay lại|Tiến|Lên|Vị trí|Nhập đường dẫn|Tìm kiếm|",
        "Địa điểm|Thư mục nhà|Gần đây|Ổ đĩa|Thư mục mới|Xem|Tệp ẩn|Hình thu nhỏ|Hủy|Mở|",
        "Lưu|Chọn thư mục này|Tên của một tệp|Một thư mục trong thư mục này, hoặc để trống cho chính nó|Tên|Kích thước|Loại|Ngày|Thư mục|",
        "Đang đọc thư mục…|Không có tên nào chứa nội dung đó|Không có gì mang tên này|đã có. Thay bằng tệp mới?|Thay thế|",
        "Tháng 1|Tháng 2|Tháng 3|Tháng 4|Tháng 5|Tháng 6|Tháng 7|Tháng 8|Tháng 9|Tháng 10|Tháng 11|Tháng 12|",
        "T2|T3|T4|T5|T6|T7|CN|",
        "SA|CH|",
    )),
    ("th", concat!(
        "ตัด|คัดลอก|วาง|เลือกทั้งหมด|เพิ่มลงในพจนานุกรม|ละเว้น|คัดลอกลิงก์|",
        "ค้นหา|",
        "ค้นหา|แทนที่ด้วย|ตรงตามตัวพิมพ์|ทั้งคำ|แทนที่|แทนที่ทั้งหมด|ไปที่บรรทัด|จาก|ถัดไป|ก่อนหน้า|ปิด|",
        "เพิ่มเติม|ปิด|เต็มพื้นที่เชื่อมต่อ|กลับไปที่เดิม|ความกว้างของบานหน้าต่าง|",
        "ไม่ใช่ตัวเลข|อย่างน้อย|อย่างมาก|",
        "เพิ่มเติม|เอาออก|ปิด|หน้าแรก|หน้าก่อนหน้า|หน้าถัดไป|หน้าสุดท้าย|เสร็จแล้ว|ปลายล่าง|ปลายบน|",
        "ชั่วโมง|นาที|วินาที|AM หรือ PM|",
        "เดือนก่อนหน้า|เดือนถัดไป|",
        "เปิดไฟล์|เปิดหลายไฟล์|บันทึกเป็น|เลือกโฟลเดอร์|ย้อนกลับ|ไปข้างหน้า|ขึ้น|ตำแหน่ง|พิมพ์เส้นทาง|ค้นหา|",
        "สถานที่|โฟลเดอร์หลัก|ล่าสุด|ไดรฟ์|โฟลเดอร์ใหม่|มุมมอง|ไฟล์ที่ซ่อน|รูปขนาดย่อ|ยกเลิก|เปิด|",
        "บันทึก|เลือกโฟลเดอร์นี้|ชื่อไฟล์|โฟลเดอร์ในโฟลเดอร์นี้ หรือเว้นว่างไว้สำหรับโฟลเดอร์นี้เอง|ชื่อ|ขนาด|ชนิด|วันที่|โฟลเดอร์|",
        "กำลังอ่านโฟลเดอร์…|ไม่มีชื่อใดที่มีข้อความนั้น|ไม่มีสิ่งใดที่ใช้ชื่อนี้|มีอยู่แล้ว แทนที่ด้วยไฟล์ใหม่หรือไม่|แทนที่|",
        "มกราคม|กุมภาพันธ์|มีนาคม|เมษายน|พฤษภาคม|มิถุนายน|กรกฎาคม|สิงหาคม|กันยายน|ตุลาคม|พฤศจิกายน|ธันวาคม|",
        "จ.|อ.|พ.|พฤ.|ศ.|ส.|อา.|",
        "AM|PM|",
    )),
    ("uk", concat!(
        "Вирізати|Копіювати|Вставити|Виділити все|Додати до словника|Пропустити|Копіювати посилання|",
        "Пошук|",
        "Знайти|Замінити на|З урахуванням регістру|Слово повністю|Замінити|Замінити все|Перейти до рядка|з|Далі|Назад|Закрити|",
        "Більше|Закрити|Заповнити область закріплення|Повернути на місце|Ширина панелі|",
        "Не число|Щонайменше|Щонайбільше|",
        "Більше|Видалити|Закрити|Перша сторінка|Попередня сторінка|Наступна сторінка|Остання сторінка|готово|нижня межа|верхня межа|",
        "Години|Хвилини|Секунди|дп або пп|",
        "Попередній місяць|Наступний місяць|",
        "Відкрити файл|Відкрити файли|Зберегти як|Вибрати папку|Назад|Вперед|Угору|Розташування|Введіть шлях|Пошук|",
        "Місця|Домашня папка|Нещодавні|Диски|Нова папка|Вигляд|Приховані файли|Ескізи|Скасувати|Відкрити|",
        "Зберегти|Вибрати цю папку|Ім’я файлу|Папка в цій папці або жодної для неї самої|Ім’я|Розмір|Тип|Дата|Папка|",
        "Читання папки…|Жодне ім’я не містить цього|Немає нічого з таким ім’ям|уже існує. Замінити новим?|Замінити|",
        "січень|лютий|березень|квітень|травень|червень|липень|серпень|вересень|жовтень|листопад|грудень|",
        "пн|вт|ср|чт|пт|сб|нд|",
        "дп|пп|",
    )),
    ("sv", concat!(
        "Klipp ut|Kopiera|Klistra in|Markera allt|Lägg till i ordlistan|Ignorera|Kopiera länk|",
        "Sök|",
        "Sök|Ersätt med|Matcha gemener/versaler|Hela ord|Ersätt|Ersätt alla|Gå till rad|av|Nästa|Föregående|Stäng|",
        "Mer|Stäng|Fyll dockningsytan|Tillbaka till sin plats|Fönsterrutans bredd|",
        "Inte ett tal|Minst|Högst|",
        "Mer|Ta bort|Stäng|Första sidan|Föregående sida|Nästa sida|Sista sidan|klar|nedre gräns|övre gräns|",
        "Timmar|Minuter|Sekunder|fm eller em|",
        "Föregående månad|Nästa månad|",
        "Öppna en fil|Öppna filer|Spara som|Välj en mapp|Bakåt|Framåt|Uppåt|Plats|Skriv en sökväg|Sök|",
        "Platser|Hemmapp|Senaste|Enheter|Ny mapp|Visa|Dolda filer|Miniatyrer|Avbryt|Öppna|",
        "Spara|Välj den här mappen|Namnet på en fil|En mapp i den här, eller ingen för den här själv|Namn|Storlek|Typ|Datum|Mapp|",
        "Läser mappen…|Inget namn innehåller det|Det finns inget med det här namnet|finns redan. Ersätta med den nya?|Ersätt|",
        "januari|februari|mars|april|maj|juni|juli|augusti|september|oktober|november|december|",
        "må|ti|on|to|fr|lö|sö|",
        "fm|em|",
    )),
];

/// The words of `language` as the system names languages ("de", "pt-BR", "de_DE.UTF-8",
/// "zh-Hans-CN"), if it has a table: by its first part; Chinese of Taiwan, Hong Kong and Macao,
/// or in traditional script, traditional.
fn words_of(language: &str) -> Option<[&'static str; WORDS]> {
    let (language, region) = crate::locales::language_and_region(language);
    let code = if language == "zh" && region.as_deref() == Some("TW") { "zh-Hant" } else { language.as_str() };
    LANGUAGES.iter().find(|(known, _)| *known == code).map(|(_, table)| split_words(table))
}

/// A language's words, from its table: one text, each word followed by "|". (One text, not a
/// list: a list's every entry would carry an address the loader fixes up, 24 bytes each on
/// Linux.)
fn split_words(table: &'static str) -> [&'static str; WORDS] {
    let mut words = table.split('|');
    std::array::from_fn(|_| words.next().unwrap_or_default())
}

/// The words of the first of the user's languages that has a table.
fn users_words() -> Option<[&'static str; WORDS]> {
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
        words_of(language).map(|words| file_words(&words))
    }
    /// The file dialog's words in the user's language; English if taut lacks it.
    pub(crate) fn users() -> FileWords {
        users_words().map_or_else(FileWords::english, |words| file_words(&words))
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
        self.take_words(&words);
        self.calendar_words.months = run(&words, MONTHS);
        self.calendar_words.days = run(&words, DAYS);
        self.calendar_words.halves = run(&words, HALVES);
        true
    }
    /// Sets the built-in words to the user's language, if taut has it, but for month and day
    /// names and AM/PM (the system's, with its formats).
    pub(crate) fn translate_for_the_user(&mut self) {
        if let Some(words) = users_words() {
            self.take_words(&words);
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
        for (code, table) in LANGUAGES {
            assert_eq!((table.split('|').count(), table.ends_with('|')), (WORDS + 1, true), "{code}: every word, each ended by |");
            let words = split_words(table);
            assert!(words.iter().all(|word| !word.trim().is_empty()), "{code}");
            // No word left in English by mistake, but those English shares.
            let english = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
            let shared = ["AM", "PM", "Minutes", "Name", "Date", "Type", "Plus", "Local", "Abrir", "Data", "Tipo", "Pasta", "Ordner"];
            let same = english.edit_words.iter().chain(&english.code_words).filter(|word| words.contains(word) && !shared.contains(word)).count();
            assert_eq!(same, 0, "{code}");
            // Months, days and halves are where the calendar takes them.
            assert!(words[MONTHS..DAYS].iter().all(|month| month.chars().count() <= 11), "{code}");
            assert!(words[DAYS..HALVES].iter().all(|day| day.chars().count() <= 3), "{code}");
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
            [Some("剪切"), Some("剪切"), Some("剪切"), Some("剪下"), Some("剪下"), Some("剪下"), None, None, None]
        );
    }

    #[test]
    fn a_window_and_a_file_dialog_take_a_language() {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.edit_words = ["Cut", "Copy", "Paste", "Select all", "Add to dictionary", "Ignore", "Copy link"];
        assert!(!ui.translate("tlh"));
        assert_eq!(ui.edit_words[0], "Cut", "nothing changes for a language taut lacks");
        assert!(ui.translate("de-CH"));
        assert_eq!((ui.edit_words[2], ui.edit_words[6], ui.search_word, ui.code_words[10]), ("Einfügen", "Link kopieren", "Suchen", "Schließen"));
        assert_eq!(ui.number_words[0], "Keine Zahl");
        assert_eq!((ui.frame_words.splitter, ui.widget_words.high, ui.calendar_words.clock[3], ui.calendar_words.arrows[1]), ("Breite des Bereichs", "oberes Ende", "AM oder PM", "Nächster Monat"));
        assert_eq!((ui.calendar_words.months[2], ui.calendar_words.days[6], ui.calendar_words.halves[1]), ("März", "So", "PM"));
        let french = FileWords::in_language("fr").unwrap();
        assert_eq!((french.open_a_file, french.replace, french.cancel), ("Ouvrir un fichier", "Remplacer", "Annuler"));
        assert_eq!(FileWords::in_language("en"), None);
        // The languages added later: AM/PM first in Korean, Arabic's shortest day names,
        // traditional Chinese by its region.
        assert!(ui.translate("ko-KR"));
        assert_eq!((ui.edit_words[0], ui.calendar_words.halves), ("잘라내기", ["오전", "오후"]));
        assert!(ui.translate("ar_EG.UTF-8"));
        assert_eq!((ui.code_words[10], ui.calendar_words.days[0], ui.calendar_words.months[0]), ("إغلاق", "ن", "يناير"));
        assert!(ui.translate("zh-HK"));
        assert_eq!(ui.edit_words[0], "剪下");
        assert_eq!(FileWords::in_language("uk-UA").map(|words| words.name), Some("Ім’я"));
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
