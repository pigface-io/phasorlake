-- ************************************************************
-- Title:	PUBLIC_DVD_P5MIN_CASESOLUTION_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_CASESOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_P5MIN_CASESOLUTION_202401010000.CSV
APPEND INTO TABLE P5MIN_CASESOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
INTERVENTION FLOAT EXTERNAL,
STARTINTERVAL_DATETIME CHAR(20),
TOTALOBJECTIVE FLOAT EXTERNAL,
NONPHYSICALLOSSES FLOAT EXTERNAL,
TOTALAREAGENVIOLATION FLOAT EXTERNAL,
TOTALINTERCONNECTORVIOLATION FLOAT EXTERNAL,
TOTALGENERICVIOLATION FLOAT EXTERNAL,
TOTALRAMPRATEVIOLATION FLOAT EXTERNAL,
TOTALUNITMWCAPACITYVIOLATION FLOAT EXTERNAL,
TOTAL5MINVIOLATION FLOAT EXTERNAL,
TOTALREGVIOLATION FLOAT EXTERNAL,
TOTAL6SECVIOLATION FLOAT EXTERNAL,
TOTAL60SECVIOLATION FLOAT EXTERNAL,
TOTALENERGYCONSTRVIOLATION FLOAT EXTERNAL,
TOTALENERGYOFFERVIOLATION FLOAT EXTERNAL,
TOTALASPROFILEVIOLATION FLOAT EXTERNAL,
TOTALFASTSTARTVIOLATION FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
