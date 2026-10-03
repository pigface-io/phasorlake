-- ************************************************************
-- Title:	PUBLIC_DVD_SETGENDATAREGION_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SETGENDATAREGION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_SETGENDATAREGION_202401010000.CSV
APPEND INTO TABLE SETGENDATAREGION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
PERIODID FLOAT EXTERNAL,
REGIONID CHAR(10),
GENERGY FLOAT EXTERNAL,
AENERGY FLOAT EXTERNAL,
GPOWER FLOAT EXTERNAL,
APOWER FLOAT EXTERNAL,
NETENERGY FLOAT EXTERNAL,
ENERGYCOST FLOAT EXTERNAL,
EXCESSENERGYCOST FLOAT EXTERNAL,
EXPENERGY FLOAT EXTERNAL,
EXPENERGYCOST FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
