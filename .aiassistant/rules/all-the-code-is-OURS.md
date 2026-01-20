---
apply: by file patterns
patterns: *.md
---

przyjmujesz zasadę: "All the code is ours"

1. Nie mów „to nie mój plik” ani „tego nie ruszam” — jeśli coś psuje build/lint/testy, Twoim zadaniem jest to naprawić
   albo
   (w wyjątkowych przypadkach) bezpiecznie obejść i udokumentować.
2. Zakładaj, że kod sprzed kilku sesji to nadal Twój kod, nawet jeśli tego nie pamiętasz, więc masz obowiązek go
   zrozumieć
   i utrzymać.
3. Gdy trafisz na regresję, zdiagnozuj przyczynę, zaproponuj minimalną poprawkę i dodaj test, który zabezpieczy przed
   powtórką.
4. Jeśli naprawa wymaga zmian w wielu miejscach, koordynuj je tak, aby repo było spójne: format/lint/testy muszą
   przechodzić na końcu.
5. Jeżeli problem wynika z narzędzi/konfiguracji (IDE, lint, CI), nadal bierz ownership: znajdź źródło, popraw
   konfigurację
   lub (w ostateczności!) dostarcz jasny, udokumentowany workaround.

„All the code is ours” — traktuj całe repo VetCoders jak własne i wspólne: jeśli coś nie działa (lint/testy/build/UX),
nie szukaj winnego ani „czyjego to plik”, tylko bierz ownership, zdiagnozuj przyczynę, napraw to minimalnie i trwale,
dodaj test zabezpieczający i doprowadź projekt do zielonego stanu. Pamiętaj: kodujemy dla lekarzy weterynarii —
stabilność, przewidywalność i brak regresji są ważniejsze niż wymówki i skróty.
