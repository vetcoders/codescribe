# ADR 2026-05-28 — Correction: Ciągły Hands-Off Dictation + Warstwowe Korekty (Sekwencyjnie)

> **Status:** PROPOSED
> **Koryguje:** ADR 2026-05-26 — Warstwowy, inkrementalny pipeline transkrypcji
> **Priorytet:** Najpierw przywrócić ciągłość transkryptu w hands-off, dopiero potem nakładać zaawansowane warstwy.

## Kontekst — Dwa odrębne bóle

### Ból nr 1 (pierwotny, najgłębszy)

W trybie **hands-off / toggle** (przeznaczonym od początku do długich, złożonych, wielominutowych wypowiedzi) system **nie buduje jednej ciągłej całości transkryptu**.

Zamiast tego:

- Traktuje wypowiedź jako serię niezależnych utterance'ów
- Podmienia / nadpisuje fragmenty transkryptu zamiast appendować
- W niektórych ścieżkach discarduje pełne audio

Efekt: użytkownik, który chce dyktować swobodnie przez 8–15 minut, dostaje pofragmentowany, niestabilny wynik. To jest dokładnie odwrotność pierwotnej intencji tego trybu.

### Ból nr 2

Nawet gdy pierwszy pass jest "w miarę dobry", późniejsze korekty (Whisper, leksykon, LLM) często przepisują duże fragmenty tekstu, który użytkownik już zobaczył. To łamie zaufanie i poczucie ciągłości ("petarda" znika).

Oba problemy istnieją równolegle. Nie da się dobrze rozwiązać drugiego, dopóki pierwszy nie jest pod kontrolą.

## Pierwotna intencja (udokumentowana)

Z dokumentacji projektu ze stycznia 2026 (pod wcześniejszą nazwą):

> Hands-off mode allows you to dictate without holding any keys. **This is ideal for longer dictation sessions**.

Porównanie w tej samej dokumentacji:

- Hold → "Quick notes, commands"
- Hands-Off → "**Longer dictation**"

Ręce-off miał być trybem, w którym użytkownik włącza, mówi swobodnie (z pauzami, namysłem), a system buduje jeden spójny transkrypt. Nie miał być "szatkowaniem" długiej wypowiedzi na osobne kawałki.

## Obecny stan vs intencja

Obecna implementacja toggle/hands-off (szczególnie non-assistive) zachowuje się jak seria krótkich, niezależnych strzałów — dokładnie tak, jakby była projektowana pod zupełnie inny przypadek użycia.

To nie jest mała rozbieżność. To jest **odwrócenie głównego przypadku użycia** trybu hands-off.

## Decyzja — Uproszczenie kontraktu

Zamiast mnożyć tryby dyktowania na wejściu (Raw / Formatting / Assistive jako osobne ścieżki nagrywania), przyjmujemy uproszczony model:

**Jeden hands-off dictation → overlay z sensownym, ciągłym transkryptem + możliwość lekkiej edycji na miejscu → trzy wyraźne akcje po nagraniu:**

- **[Format]** — sformatuj / wypoleruj (AI formatting)
- **[Copy]** — po prostu skopiuj
- **[Agent]** — wyślij do agenta (dawne Augment / Assistive)

Dictation sam w sobie jest jeden (dla non-assistive hands-off). Różnicowanie zachowań dzieje się **po** nagraniu, nie podczas niego.

To jest ucywilizowanie kontraktu, który obrosł historycznym gównem.

## Sztywny plan wykonania (kolejność nienegocjowalna)

### Faza 1 — Przywrócić ciągłość append w hands-off (non-assistive long-form)

**Cel:** W trybie hands-off (non-assistive, RAW lub z opcjonalnym formattingiem) system musi budować **jeden ciągły transkrypt** przez całą sesję, zamiast podmieniać fragmenty co utterance.

**Co musi się stać (minimum):**

- W non-assistive ścieżce toggle/hands-off: `append_mode` musi być włączone dla całego nagrania.
- `handle_toggle_utterance` (lub równoważny mechanizm) nie może już uruchamiać pełnego pipeline'u z efektem podmiany na overlay i w transkrypcie.
- `stop_toggle_recording` musi przestać discardować WAV w ścieżce non-assistive long-form.
- Po stopie: jeden spójny transkrypt + pełne audio idą do `process_stopped_recording` (lub równoważnego miejsca).
- Overlay w trakcie nagrywania powinien pokazywać rosnący, appendowany tekst (z ewentualnymi lekkimi, widocznymi korektami, ale nie masowymi podmianami).

**Akceptacja wymagana przed jakąkolwiek dalszą pracą:**

Operator musi explicite potwierdzić (słownie lub na piśmie), że:

- W hands-off (non-assistive) po tej zmianie faktycznie dostaje jeden ciągły, appendowany transkrypt.
- Różnica między "jak wygląda tekst podczas mówienia" a "jak wygląda tuż przed wklejeniem" jest akceptowalna.
- Mechanizm nie łamie istniejących flow (jeśli ktoś jeszcze polega na starym per-utterance zachowaniu).

**Dopiero po tym potwierdzeniu** można przechodzić dalej.

### Faza 2 — Warstwowy model korekty (dopiero po akceptacji Fazy 1)

Dopiero gdy Faza 1 jest zaakceptowana i stabilna, można wprowadzać:

- Apple jako główny live engine (Warstwa 0)
- Whisper Tail Patch jako suplement (Warstwa 1)
- Leksykon + mały LLM (Warstwa 2)
- Paralingual annotations (Warstwa 3)
- Final BAM (Warstwa 4)

Z zachowaniem wszystkich twardych niezmienników z ADR 2026-05-26, szczególnie "NIGDY NIE PRZEPISUJ OD ZERA".

## Twarde niezmienniki tej korekty

1. **Kolejność jest święta.** Faza 2 nie rusza się przed jawnym akceptem Fazy 1.
2. **Hands-off (non-assistive) ma być ciągły.** Domyślnie nie szatkuje długich wypowiedzi na osobne uterrance'y z efektem podmiany.
3. **Overlay dostaje sensowny transkrypt.** Nie surowy strumień błędów, ale coś, co użytkownik może czytać i lekko edytować na bieżąco.
4. **Różnicowanie przez akcje, nie przez tryby nagrywania.** [Format] / [Copy] / [Agent] zastępują wcześniejsze rozgałęzienia na poziomie samego dictation.

## Dla agenta wykonującego tę pracę

Jeśli dostajesz ten dokument jako zadanie:

- Twoim pierwszym i jedynym zadaniem na początku jest **Faza 1**.
- Nie ruszaj Apple, Whisper tail patch, layered orchestratora ani niczego z ADR 2026-05-26, dopóki nie dostaniesz wyraźnego "tak, Faza 1 działa i jest zaakceptowana".
- Najpierw zlokalizuj wszystkie miejsca, w których toggle/hands-off (non-assistive) uruchamia per-utterance pipeline z efektem podmiany w overlayu i w transkrypcie.
- Zaproponuj minimalną, bezpieczną zmianę, która sprawia, że non-assistive hands-off zaczyna appendować zamiast podmieniać.
- Po zmianie przygotuj jasne kryteria akceptacji dla operatora.
- Dopiero po dostaniu zielonego światła przechodź do czegokolwiek związanego z warstwowym modelem.

Naruszenie kolejności = złamanie tego dokumentu.

## Konsekwencje

Po zrobieniu tego porządku (najpierw ciągłość, potem korekty) dostajemy:

- Znacznie prostszy i bardziej przewidywalny kontrakt dla użytkownika w rękach-off.
- Overlay, który faktycznie pomaga przy długim mówieniu, zamiast wprowadzać w błąd.
- Możliwość nałożenia później zaawansowanego layered modelu na już stabilny fundament, zamiast na obecny bałagan.

---

**Data:** 2026-05-28
**Autor:** Operator
**Status:** PROPOSED — wymaga implementacji w ścisłej kolejności z gate'em po Fazie 1.

## Revision 2026-06-11: Format is in-overlay

The Phase 1 assumption that `[Format]` closes the transcription overlay immediately is superseded by C5
(`bolaczki-8/dispatch/C5_prompt.md`). `[Format]` now keeps the overlay open, shows an in-flight formatting
state, returns the formatted result into the same editable text view, and exposes `[Paste]`, `[Copy]`, and
`[Close]` actions with auto-hide disabled for the formatted result.
